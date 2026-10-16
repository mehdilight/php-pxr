//! Native M7 source acceptance through generated resumable frames.
use std::path::PathBuf;
use std::process::{Command, Output, Stdio};
use std::time::{Duration, Instant};

fn execute(command: &mut Command) -> Output {
    let mut child = command
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(30);
    while child.try_wait().unwrap().is_none() {
        if Instant::now() >= deadline {
            child.kill().unwrap();
            let out = child.wait_with_output().unwrap();
            panic!(
                "async program timed out: {}",
                String::from_utf8_lossy(&out.stderr)
            );
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    child.wait_with_output().unwrap()
}

fn have_cc() -> bool {
    Command::new(std::env::var("CC").unwrap_or_else(|_| "clang".into()))
        .arg("--version")
        .output()
        .is_ok_and(|o| o.status.success())
}
fn build(name: &str, source: &str, opt: u8) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("pxr-m7-{}-{name}-{opt}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let input = dir.join("main.php");
    let output = dir.join("main");
    std::fs::write(&input, source).unwrap();
    let built = Command::new(env!("CARGO_BIN_EXE_pxr"))
        .arg("build")
        .arg(&input)
        .arg("--opt")
        .arg(opt.to_string())
        .arg("--keep-llvm")
        .arg("-o")
        .arg(&output)
        .output()
        .unwrap();
    assert!(
        built.status.success(),
        "{name}: {}",
        String::from_utf8_lossy(&built.stderr)
    );
    if std::env::var_os("PXR_RUNTIME_SANITIZERS").is_some() {
        let runtime = dir.join("runtime.c");
        let worker = include_str!("../../../runtime/pxr_php_worker.php")
            .trim_start_matches("<?php")
            .as_bytes()
            .iter()
            .map(|b| format!("\\{b:03o}"))
            .collect::<String>();
        std::fs::write(
            &runtime,
            format!(
                "{}\nconst char pxr_php_worker_source[] = \"{}\";\n",
                pxr_driver::RUNTIME_SOURCE,
                worker
            ),
        )
        .unwrap();
        let linked = Command::new(std::env::var("CC").unwrap_or_else(|_| "clang".into()))
            .args([
                "-fsanitize=address,undefined",
                "-fno-omit-frame-pointer",
                "-g",
            ])
            .arg(format!("-O{opt}"))
            .arg(output.with_extension("ll"))
            .arg(runtime)
            .args(["-lm", "-o"])
            .arg(&output)
            .output()
            .unwrap();
        assert!(
            linked.status.success(),
            "{}",
            String::from_utf8_lossy(&linked.stderr)
        );
    }
    output
}

fn run_case(name: &str, source: &str, expected: &str) {
    if !have_cc() {
        return;
    }
    for opt in [0, 3] {
        let executable = build(name, source, opt);
        for stress in [false, true] {
            let mut command = Command::new(&executable);
            command.env("PXR_PHP", "/pxr-no-php-cli");
            if stress {
                command.env("PXR_GC_STRESS", "1");
            }
            let result = execute(&mut command);
            assert!(
                result.status.success(),
                "{name} opt={opt} stress={stress}: {}",
                String::from_utf8_lossy(&result.stderr)
            );
            assert_eq!(
                result.stdout,
                expected.as_bytes(),
                "{name} opt={opt} stress={stress}"
            );
            assert!(
                result.stderr.is_empty(),
                "{}",
                String::from_utf8_lossy(&result.stderr)
            );
        }
        std::fs::remove_dir_all(executable.parent().unwrap()).unwrap();
    }
}
#[test]
fn lazy_tasks_spawn_ordered_awaits_and_implicit_joins() {
    run_case(
        "schedule",
        r#"<?php
async function work(int $id): int {
    echo "start", $id, ":";
    await sleepAsync(0);
    echo "end", $id, ":";
    return $id;
}
async function group(): int {
    spawn work(3);
    echo "parent:";
    return 30;
}
async function main(): void {
    $unused = work(99);
    work(97); // discard a lazy task without running its body
    $dropped = work(98); drop($dropped);
    $a = spawn work(1); $b = spawn work(2);
    echo await $a, ":", await $b, ":";
    echo await group(), ":done\n";
}
"#,
        "start1:start2:end1:end2:1:2:parent:start3:end3:30:done\n",
    );
}
#[test]
fn await_preserves_expression_order_and_control_flow() {
    run_case(
        "control",
        r#"<?php
function combine(string $a, string $b): string { return $a . $b; }
async function text(int $i): string {
    await sleepAsync(0);
    return "v" . $i;
}
async function main(): void {
    $out = "";
    for ($i = 0; $i < 4; $i++) {
        if ($i % 2 == 0) { $out .= await text($i); }
        else { $out .= combine("[", await text($i)) . "]"; }
    }
    foreach ([1, 2] as $n) { await sleepAsync(0); $out .= $n; }
    $n = 0;
    while ($n < 2) { echo await text($n), ":"; $n++; }
    echo false && (await text(99) == "x") ? "bad" : "ok", ":";
    echo true ? await text(7) : await text(8), ":";
    echo combine(str_repeat("a", 100), await text(4)), ":", $out, "\n";
}
"#,
        &format!("v0:v1:ok:v7:{}v4:v0[v1]v2[v3]12\n", "a".repeat(100)),
    );
}
#[test]
fn suspended_owned_values_arrays_objects_mixed_and_scalar_results() {
    run_case(
        "values",
        r#"<?php
class Counter {
    public int $value;
    public function __construct(int $n) { $this->value = $n; }
}
function read(&Counter $c): int { return $c->value; }
async function consume(owned<Counter> $c): int {
    await sleepAsync(0);
    return read(&$c);
}
async function arrayValue(array<int> $values): array<int> {
    await sleepAsync(0); $values[] = 3; return $values;
}
async function objectValue(): Counter { await sleepAsync(0); return new Counter(8); }
async function mixedValue(): mixed { $m: mixed = ["x" => "hello"]; await sleepAsync(0); return $m; }
async function floatValue(float $x): float { await sleepAsync(0); return $x; }
async function boolValue(bool $x): bool { await sleepAsync(0); return $x; }
async function main(): void {
    $c: owned<Counter> = new Counter(42);
    $pending = consume(move($c));
    await sleepAsync(0);
    echo await $pending, ":";
    $input = [1, 2]; $values = await arrayValue($input);
    echo count($input), ":", implode(",", $values), ":";
    $object = await objectValue(); echo $object->value, ":";
    $m = await mixedValue(); if (is_array($m)) { echo $m["x"], ":"; }
    echo await floatValue(-0.5), ":", await boolValue(true), "\n";
}
"#,
        "42:2:1,2,3:8:hello:-0.5:1\n",
    );
}
#[test]
fn repeated_awaits_reclaim_frame_values_and_results() {
    if !have_cc() {
        return;
    }
    let source = r#"<?php
async function make(): string { await sleepAsync(0); return str_repeat("x", 4096); }
async function main(): void {
    $total = 0;
    for ($i = 0; $i < 1200; $i++) { $text = await make(); $total += strlen($text); }
    echo $total, "\n";
}
"#;
    for opt in [0, 3] {
        let executable = build("retention", source, opt);
        let out = execute(Command::new(&executable).env("PXR_GC_STATS", "1"));
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        assert_eq!(out.stdout, b"4915200\n");
        let stderr = String::from_utf8_lossy(&out.stderr);
        let peak: usize = stderr
            .split("peak_bytes=")
            .nth(1)
            .unwrap()
            .split_whitespace()
            .next()
            .unwrap()
            .parse()
            .unwrap();
        assert!(peak < 2 * 1024 * 1024, "{stderr}");
        assert!(
            !stderr.contains("Sanitizer") && !stderr.contains("runtime error:"),
            "{stderr}"
        );
        std::fs::remove_dir_all(executable.parent().unwrap()).unwrap();
    }
}
#[test]
fn async_failure_drains_children_and_reports_error() {
    if !have_cc() {
        return;
    }
    let source = r#"<?php
async function slow(): void { echo "started:"; await sleepAsync(10000000); echo "bad"; }
async function main(): void { spawn slow(); await sleepAsync(-1); echo "bad"; }
"#;
    for opt in [0, 3] {
        let executable = build("failure", source, opt);
        let out = execute(Command::new(&executable).env("PXR_GC_STRESS", "1"));
        assert_eq!(out.status.code(), Some(255));
        assert_eq!(out.stdout, b"started:");
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(stderr.starts_with("pxr: async task failed:"), "{stderr}");
        assert!(
            !stderr.contains("Sanitizer") && !stderr.contains("runtime error:"),
            "{stderr}"
        );
        std::fs::remove_dir_all(executable.parent().unwrap()).unwrap();
    }
}
#[test]
fn invalid_async_forms_fail_before_codegen() {
    let common = "async function value(): int { await sleepAsync(0); return 1; }\n";
    let cases = [
        ("outside", "await value();".to_string()),
        ("sync-call", "function f(): void { value(); }".to_string()),
        ("entry-params", "async function main(int $x): void {}".to_string()),
        ("entry-result", "async function main(): int { return 1; }".to_string()),
        ("entry-top", "echo 1; async function main(): void {}".to_string()),
        ("generic", "async function f<T>(T $x): T { return $x; }".to_string()),
        ("inferred", "async function f() { return 1; }".to_string()),
        ("borrow-capture", "async function f(&int $x): int { return $x; }".to_string()),
        ("owned-result", "async function f(): owned<int> { return 1; }".to_string()),
        ("invalid-operand", "async function main(): void { await 1; }".to_string()),
        ("double-await", "async function main(): void { $t = value(); await $t; await $t; }".to_string()),
        ("double-spawn", "async function main(): void { $t = spawn value(); spawn $t; }".to_string()),
        ("respun-after-reassignment", "async function main(): void { $t = value(); await $t; $t = spawn value(); spawn $t; }".to_string()),
        ("conditional-respawn", "async function main(): void { if (true) { $t = value(); } else { $t = spawn value(); } spawn $t; }".to_string()),
        ("void-value", "async function main(): void { $x = await sleepAsync(0); }".to_string()),
        ("async-method", "class X { public async function f(): int { return 1; } }".to_string()),
        ("borrowed-after-move", "class X {} async function f(owned<X> $x): void {} async function main(): void { $x: owned<X> = new X(); $t = f(move($x)); $x; }".to_string()),
        ("overwrite-live-task", "async function main(): void { $t = value(); $t = value(); }".to_string()),
        ("bare-task", "async function main(): void { $t = value(); $t; }".to_string()),
        ("alias", "async function main(): void { $t = value(); $alias = $t; }".to_string()),
        ("escape", "async function main(): void { $a = [value()]; }".to_string()),
        ("return-task", "async function f(): mixed { return value(); }".to_string()),
        ("echo-void", "async function main(): void { echo await sleepAsync(0); }".to_string()),
        ("print-void", "async function main(): void { print await sleepAsync(0); }".to_string()),
        ("print-task", "async function main(): void { echo value(); }".to_string()),
        ("cast-task", "async function main(): void { $t = value(); echo intval($t); }".to_string()),
        ("mixed-op", "async function main(): void { $m: mixed = 1; echo value() == $m; }".to_string()),
        ("task-to-mixed", "async function main(): void { $t: mixed = value(); }".to_string()),
        ("borrow-across-await", "function read(&int $a, int $b): int { return $a + $b; } async function main(): void { $x: owned<int> = 1; echo read(&$x, await value()); }".to_string()),
        ("task-generic", "function id<T>(owned<T> $x): owned<T> { return move($x); } async function main(): void { $t = value(); $other = id(move($t)); }".to_string()),
    ];
    let dir = std::env::temp_dir().join(format!("pxr-m7-errors-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    for (name, invalid) in cases {
        let input = dir.join(format!("{name}.php"));
        std::fs::write(&input, format!("<?php\n{common}{invalid}")).unwrap();
        let out = Command::new(env!("CARGO_BIN_EXE_pxr"))
            .arg("check")
            .arg(&input)
            .output()
            .unwrap();
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(!out.status.success(), "{name} unexpectedly accepted");
        assert!(!stderr.contains("panicked"), "{name}: {stderr}");
        assert!(stderr.contains("error"), "{name}: {stderr}");
    }
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn affine_tasks_move_reinitialize_and_join_across_branches() {
    run_case(
        "affine",
        r#"<?php
async function value(int $n): int { await sleepAsync(0); return $n; }
async function pick(bool $spawned): int {
    if ($spawned) { $t = spawn value(5); } else { $t = value(6); }
    return await $t;
}
async function main(): void {
    $first = value(1); $moved = move($first);
    $first = spawn value(2); $joined = spawn $moved;
    echo await $joined, ":", await $first, ":", await pick(true), ":", await pick(false), "\n";
}
"#,
        "1:2:5:6\n",
    );
}
#[test]
fn async_functions_resolve_across_native_modules_and_namespaces() {
    if !have_cc() {
        return;
    }
    let dir = std::env::temp_dir().join(format!("pxr-m7-modules-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("helpers.php"),
        "<?php namespace Jobs; async function answer(): int { await sleepAsync(0); return 42; }",
    )
    .unwrap();
    std::fs::write(dir.join("main.php"), "<?php require __DIR__ . '/helpers.php'; use function Jobs\\answer; async function main(): void { echo await answer(), \"\\n\"; }").unwrap();
    let executable = dir.join("main");
    let out = Command::new(env!("CARGO_BIN_EXE_pxr"))
        .arg("build")
        .arg(dir.join("main.php"))
        .arg("-o")
        .arg(&executable)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let out = execute(Command::new(executable).env("PXR_GC_STRESS", "1"));
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(out.stdout, b"42\n");
    assert!(out.stderr.is_empty());
    std::fs::remove_dir_all(dir).unwrap();
}
