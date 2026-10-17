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
    if std::env::var_os("PXR_RUNTIME_SANITIZERS").is_some()
        || std::env::var_os("PXR_RUNTIME_THREAD_SANITIZER").is_some()
    {
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
                if std::env::var_os("PXR_RUNTIME_THREAD_SANITIZER").is_some() {
                    "-fsanitize=thread"
                } else {
                    "-fsanitize=address,undefined"
                },
                "-fno-omit-frame-pointer",
                "-g",
            ])
            .arg(format!("-O{opt}"))
            .arg(output.with_extension("ll"))
            .arg(runtime)
            .args(["-pthread", "-lm", "-o"])
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
fn cow_arrays_keep_task_captures_and_results_independent() {
    run_case(
        "cow_tasks",
        r#"<?php
async function change(array<array<int>> $values): array<array<int>> {
    await sleepAsync(0);
    $child = $values[0]; $child[0] = 42; $values[0] = $child;
    return $values;
}
async function main(): void {
    $original = [[7, 8], [9]];
    $task = spawn change($original);
    $child = $original[0]; $child[0] = 10; $original[0] = $child;
    await sleepAsync(0);
    $result = await $task; $copy = $result;
    $child = $result[0]; $child[0] = 99; $result[0] = $child;
    $before = $original[0]; $after = $copy[0]; $changed = $result[0];
    echo $before[0], ":", $after[0], ":", $changed[0], "\n";
}
"#,
        "10:42:99\n",
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
        ("io-sync", "function f(): void { tcpExchangeAsync(\"127.0.0.1\", 80, \"\", 10); }".to_string()),
        ("io-arity", "async function main(): void { await tcpExchangeAsync(\"127.0.0.1\", 80, \"\"); }".to_string()),
        ("io-type", "async function main(): void { await tcpExchangeAsync(1, 80, \"\", 10); }".to_string()),
        ("io-timeout-type", "async function main(): void { await tcpExchangeAsync(\"127.0.0.1\", 80, \"\", 10, \"bad\"); }".to_string()),
        ("await-borrowed-selection", "async function main(): void { $a = value(); $b = value(); await (true ? $a : $b); await $a; }".to_string()),
        ("spawn-borrowed-selection", "async function main(): void { $a = value(); $b = value(); spawn (true ? $a : $b); }".to_string()),
        ("cancel-borrowed-selection", "async function main(): void { $a = value(); $b = value(); cancelTask(true ? $a : $b); await $a; }".to_string()),
        ("cancel-reuse", "async function main(): void { $t = value(); cancelTask($t); await $t; }".to_string()),
        ("cancel-twice", "async function main(): void { $t = value(); cancelTask($t); cancelTask($t); }".to_string()),
        ("cancel-value", "async function main(): void { cancelTask(1); }".to_string()),
        ("cancel-outside", "cancelTask(value());".to_string()),
        ("reserve-io", "function tcpExchangeAsync(): void {}".to_string()),
        ("reserve-cancel", "function cancelTask(): void {}".to_string()),
        ("outside", "await value();".to_string()),
        ("sync-call", "function f(): void { value(); }".to_string()),
        ("entry-params", "async function main(int $x): void {}".to_string()),
        ("entry-result", "async function main(): int { return 1; }".to_string()),
        ("entry-top", "echo 1; async function main(): void {}".to_string()),
        ("generic-main", "async function main<T>(): void {}".to_string()),
        ("inferred-recursion", "async function f() { return await f(); }".to_string()),
        ("borrow-capture", "async function f(&int $x): int { return $x; }".to_string()),
        ("owned-result-copy", "async function f(owned<int> $x): owned<int> { return $x; }".to_string()),
        ("invalid-operand", "async function main(): void { await 1; }".to_string()),
        ("double-await", "async function main(): void { $t = value(); await $t; await $t; }".to_string()),
        ("double-spawn", "async function main(): void { $t = spawn value(); spawn $t; }".to_string()),
        ("respun-after-reassignment", "async function main(): void { $t = value(); await $t; $t = spawn value(); spawn $t; }".to_string()),
        ("conditional-respawn", "async function main(): void { if (true) { $t = value(); } else { $t = spawn value(); } spawn $t; }".to_string()),
        ("void-value", "async function main(): void { $x = await sleepAsync(0); }".to_string()),
        ("async-constructor", "class X { public async function __construct() {} }".to_string()),
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
fn async_methods_inference_dispatch_and_owned_results_work_together() {
    run_case(
        "async-methods-inference",
        r#"<?php
interface Service { public async function value(int $x): string; }
class Base implements Service {
    public async function value(int $x): string { await sleepAsync(0); return "base" . $x; }
    public static async function id<T>(T $x) { await sleepAsync(0); return $x; }
}
class Child extends Base {
    public async function value(int $x): string { return "child:" . await parent::value($x); }
}
class Box<T> {
    public T $value;
    public function __construct(T $value) { $this->value = $value; }
    public async function read() { await sleepAsync(0); return $this->value; }
}
async function object() { await sleepAsync(0); return new Child(); }
async function make<T>(T $x): owned<Box<T>> { await sleepAsync(0); return new Box<T>($x); }
async function transfer<T>(owned<T> $x): owned<T> { await sleepAsync(0); return move($x); }
async function main() {
    $s: Service = new Child(); $task = $s->value(4); echo await $task, ":";
    echo await Base::id("static"), ":";
    $obj = await object(); echo await $obj->value(5), ":";
    $owner = await make(42); $job = spawn transfer(move($owner));
    $next = await $job; echo await move($next)->read(), "\n";
}"#,
        "child:base4:static:child:base5:42\n",
    );
}

#[test]
fn generic_async_methods_dispatch_through_interfaces_traits_and_inheritance() {
    run_case(
        "async-generic-methods",
        r#"<?php
interface Echoer { public async function echoValue<T>(T $x): T; }
trait DelayedEcho {
    public async function echoValue<U>(U $x): U { await sleepAsync(0); return $x; }
}
class First implements Echoer { use DelayedEcho; }
class Second extends First {
    public async function echoValue<V>(V $x): V { return await parent::echoValue<V>($x); }
}
class Factory {
    public static async function create() { await sleepAsync(0); return new Second(); }
}
async function main(): void {
    $a: Echoer = new First(); $b: Echoer = new Second();
    $one = spawn $a->echoValue(41); $two = spawn $b->echoValue<string>("two");
    echo await $one, ":", await $two, ":";
    $created = await Factory::create(); echo await $created->echoValue("three"), "\n";
}"#,
        "41:two:three\n",
    );
}

#[test]
fn async_return_inference_handles_recursive_anchors_widening_and_arrays() {
    run_case(
        "async-return-inference",
        r#"<?php
async function later<T>(T $x) { await sleepAsync(0); return $x; }
async function recursive(int $n) { if ($n === 0) { return 7; } return await recursive($n - 1); }
async function number(bool $int) { if ($int) { return 1; } return await later(2.5); }
class Values {
    public async function get(bool $empty) {
        await sleepAsync(0); if ($empty) { return []; } return ["value"];
    }
}
async function main() {
    $v = new Values();
    echo await recursive(4), ":", await number(true) + 0.5, ":", await number(false), ":";
    $a = await $v->get(false); echo $a[0], ":", count(await $v->get(true)), "\n";
}"#,
        "7:1.5:2.5:value:0\n",
    );
}

#[test]
fn invalid_async_method_contracts_and_receiver_escapes_are_diagnosed() {
    let dir = std::env::temp_dir().join(format!("pxr-async-method-errors-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    for (i, source) in [
        "class X { public async function __construct() {} }",
        "class X { public async function __toString(): string { return \"x\"; } }",
        "class A { public async function f(): int { return 1; } } class B extends A { public function f(): int { return 1; } }",
        "interface A { public async function f(): int; } class B implements A { public function f(): int { return 1; } }",
        "class X { public async function f(&int $x): int { return $x; } }",
        "class X { public async function f(): int { return 1; } } $x = new X(); $x->f();",
        "class X { public static async function f(): int { return 1; } } X::f();",
        "class X { public async function f(): int { return 1; } } async function main(): void { $x: owned<X> = new X(); await $x->f(); }",
        "class X { public async function f(): int { return 1; } } async function main(): void { $x: owned<X> = new X(); $t = move($x)->f(); echo $x; }",
        "class X { public async function f(): int { return 1; } } async function main(): void { $x = new X(); $t = $x->f(); await $t; await $t; }",
        "async function make(): owned<int> { return 1; } async function main(): void { $x = await make(); $a = move($x); echo $x; }",
        "async function f() { return await f(); }",
        "class X { public async function f() { return await $this->f(); } }",
        "async function main() { return 1; }",
    ].iter().enumerate() {
        let path = dir.join(format!("{i}.php")); std::fs::write(&path, format!("<?php {source}")).unwrap();
        let out = Command::new(env!("CARGO_BIN_EXE_pxr")).arg("check").arg(path).output().unwrap();
        assert!(!out.status.success(), "case {i} accepted");
        assert!(!String::from_utf8_lossy(&out.stderr).contains("panicked"));
    }
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn generic_async_calls_preserve_results_recursion_and_lazy_task_rules() {
    run_case(
        "generic-async-values",
        r#"<?php
function syncIdentity<T>(T $value) { return $value; }
async function identity<T>(T $value): T { await sleepAsync(0); return syncIdentity($value); }
async function even<T>(T $value, int $n): T {
    if ($n === 0) { return $value; }
    return await odd($value, $n - 1);
}
async function odd<T>(T $value, int $n): T {
    if ($n === 0) { return $value; }
    return await even<T>($value, $n - 1);
}
async function emit<T>(T $value): void { await sleepAsync(0); echo $value; }
async function never<T>(T $value): void { echo "unreachable"; await sleepAsync(10000000); }
async function main(): void {
    $lazy = never("discarded"); drop($lazy);
    $first = identity(42); $moved = move($first); $started = spawn $moved;
    echo await $started, ":", await identity("hello"), ":";
    echo await identity<float>(1) + 0.5, ":", await identity(true), ":";
    echo is_null(await identity(null)), ":";
    $mixed = await identity<mixed>("mixed");
    echo strval($mixed), ":", await even("recursive", 7), ":";
    await emit<int>(9); echo "\n";
}"#,
        "42:hello:1.5:1:1:mixed:recursive:9\n",
    );
}

#[test]
fn generic_async_arrays_objects_and_owned_captures_survive_suspension() {
    run_case(
        "generic-async-roots",
        r#"<?php
class Box<T> {
    public T $value;
    public function __construct(T $value) { $this->value = $value; }
}
async function identity<T>(T $value): T { await sleepAsync(0); return $value; }
async function first<T>(array<T> $values, T $fallback): T {
    await sleepAsync(0); return $values[0] ?? $fallback;
}
async function consume<T>(owned<Box<T>> $box): T {
    await sleepAsync(0); return $box->value;
}
async function ownedFirst<T>(owned<array<T>> $values): T {
    await sleepAsync(0); return $values[0];
}
async function held<T>(owned<T> $value): void { await sleepAsync(10000000); }
async function main(): void {
    echo await first([], 7), ":", await first(["a", "b"], "z"), ":";
    $empty = await identity([]); echo count($empty), ":";
    $original = [1, 2]; $pending = identity($original); $original[0] = 9;
    $result = await $pending; $result[1] = 8; echo $original[1], ":", $result[0], ":";
    $array = await identity<array<mixed>>([1, "two"]);
    echo count($array), ":", strval($array[1]), ":";
    $object = new Box<string>(str_repeat("x", 128));
    $copy = await identity($object); $copy->value = "shared";
    echo $object->value, ":";
    $owner: owned<Box<string>> = new Box<string>(str_repeat("y", 128));
    $task = consume(move($owner)); await sleepAsync(0);
    echo strlen(await $task), ":";
    $values: owned<array<string>> = [str_repeat("z", 128)];
    echo strlen(await ownedFirst(move($values))), "\n";
    $capture: owned<Box<string>> = new Box<string>(str_repeat("c", 8192));
    $cancelled = spawn held(move($capture)); await sleepAsync(0); cancelTask($cancelled);
}"#,
        "7:a:0:2:1:2:two:shared:128:128\n",
    );
}

#[test]
fn generic_async_bounds_and_specializations_reuse_concrete_native_functions() {
    if !have_cc() {
        return;
    }
    let source = r#"<?php
interface Named { public function name(): string; }
class Label implements Named {
    public function name(): string { return "label"; }
}
async function describe<T: Named>(T $value): string {
    await sleepAsync(0); return $value->name();
}
async function identity<T>(T $value): T { await sleepAsync(0); return $value; }
async function unused<T>(T $value): T { return $value->missing(); }
async function main(): void {
    echo await identity(1), ":", await identity<int>(2), ":";
    echo await identity("a"), ":", await identity<string>("b"), ":";
    echo await describe(new Label()), "\n";
}"#;
    let encoded: String = b"identity".iter().map(|b| format!("{b:02x}")).collect();
    let unused: String = b"unused".iter().map(|b| format!("{b:02x}")).collect();
    let prefix = ["pxr$g$", &encoded, "$"].concat();
    let unused_prefix = ["pxr$g$", &unused, "$"].concat();
    for opt in [0, 3] {
        let executable = build("generic-async-specializations", source, opt);
        let llvm = std::fs::read_to_string(executable.with_extension("ll")).unwrap();
        let definitions: Vec<_> = llvm
            .lines()
            .filter(|line| line.starts_with("define ") && line.contains(&prefix))
            .collect();
        assert_eq!(
            definitions.len(),
            4,
            "two constructors and their resumable bodies: {definitions:?}"
        );
        assert!(!llvm.contains(&unused_prefix));
        for stress in [false, true] {
            let mut command = Command::new(&executable);
            command.env("PXR_PHP", "/pxr-no-php-cli");
            if stress {
                command.env("PXR_GC_STRESS", "1");
            }
            let out = execute(&mut command);
            assert!(
                out.status.success(),
                "{}",
                String::from_utf8_lossy(&out.stderr)
            );
            assert_eq!(out.stdout, b"1:2:a:b:label\n");
            assert!(out.stderr.is_empty());
        }
        std::fs::remove_dir_all(executable.parent().unwrap()).unwrap();
    }
}

#[test]
fn generic_async_invalid_calls_captures_and_specializations_are_diagnosed() {
    let dir = std::env::temp_dir().join(format!("pxr-generic-async-errors-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    for (i, (source, diagnostic)) in [
        ("async function f<T>(T $x): T { return $x; } function g(): void { f(1); }", "E0300"),
        ("async function f<T>(T $x): T { return $x; } $t = f(1);", "E0300"),
        ("async function main<T>(): void {}", "E0300"),
        ("async function f<T>(T $x) { return await f($x); } async function main(): void { await f(1); }", "E0284"),
        ("async function f<T>(&T $x): T { return $x; }", "call-scoped borrows"),
        ("async function f<T>(&mut array<T> $x): void {}", "call-scoped borrows"),
        ("async function f<T>(owned<T> $x): owned<T> { return $x; }", "owned return"),
        ("async function f<T>(T $x): T { return $x; } async function main(): void { $t = f(1); await $t; await $t; }", "error"),
        ("async function f<T>(T $x): T { return $x; } async function main(): void { $a = [f(1)]; }", "error"),
        ("async function f<T>(T $x): T { return $x; } async function main(): void { echo await f(sleepAsync(0)); }", "error"),
        ("async function f<T>(T $x): T { return $x; } async function main(): void { $x: owned<int> = 1; await f(&$x); }", "error"),
        ("async function f<T>(owned<T> $x): void {} async function main(): void { $x: owned<string> = \"x\"; $t = f(move($x)); echo $x; }", "error"),
        ("function read(&int $x, int $y): int { return $x + $y; } async function f<T>(T $x): T { return $x; } async function main(): void { $x: owned<int> = 1; echo read(&$x, await f(1)); }", "error"),
        ("async function f<T>(T $x, T $y): T { return $x; } async function main(): void { await f(1, 2.5); }", "E0281"),
        ("async function f<T>(T $x): T { return $x; } async function main(): void { await f<int,string>(1); }", "E0280"),
        ("async function f<T>(T $x): T { return $x; } async function main(): void { await f<void>(1); }", "E0280"),
        ("async function f<T>(array<T> $x): T { return $x[0]; } async function main(): void { await f([]); }", "E0281"),
        ("interface Named { public function name(): string; } async function f<T: Named>(T $x): T { return $x; } async function main(): void { await f(1); }", "error"),
        ("async function f<T>(T $x): T { return $x->missing(); } async function main(): void { await f(1); }", "while specializing"),
        ("async function grow<T>(T $x): void { await grow([$x]); } async function main(): void { await grow(1); }", "E0283"),
        ("async function f<T>(T $x): T { return $x; } async function main(): void { await tcpServeAsync(\"127.0.0.1\", 8080, \"f\", 1, 10); }", "E0300"),
    ].iter().enumerate() {
        let path = dir.join(format!("{i}.php")); std::fs::write(&path, format!("<?php {source}")).unwrap();
        let out = Command::new(env!("CARGO_BIN_EXE_pxr")).arg("check").arg(path).output().unwrap();
        let error = String::from_utf8_lossy(&out.stderr);
        assert!(!out.status.success(), "case {i} accepted");
        assert!(error.contains(diagnostic) && !error.contains("panicked"), "case {i}: {error}");
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
        "<?php namespace Jobs; async function answer(): int { await sleepAsync(0); return 42; } async function later<T>(T $value): T { await sleepAsync(0); return $value; }",
    )
    .unwrap();
    std::fs::write(dir.join("main.php"), "<?php require __DIR__ . '/helpers.php'; use function Jobs\\answer; use function Jobs\\later as delayed; async function main(): void { echo await answer(), \":\", await delayed<string>(\"hello\"), \"\\n\"; }").unwrap();
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
    assert_eq!(out.stdout, b"42:hello\n");
    assert!(out.stderr.is_empty());
    std::fs::remove_dir_all(dir).unwrap();
}

fn accept_tcp(listener: &std::net::TcpListener) -> std::net::TcpStream {
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        match listener.accept() {
            Ok((stream, _)) => {
                stream.set_nonblocking(false).unwrap();
                stream
                    .set_read_timeout(Some(Duration::from_secs(5)))
                    .unwrap();
                stream
                    .set_write_timeout(Some(Duration::from_secs(5)))
                    .unwrap();
                return stream;
            }
            Err(error)
                if error.kind() == std::io::ErrorKind::WouldBlock && Instant::now() < deadline =>
            {
                std::thread::sleep(Duration::from_millis(5))
            }
            Err(error) => panic!("TCP fixture accept: {error}"),
        }
    }
}
#[test]
fn tcp_exchange_preserves_binary_payloads_and_makes_concurrent_progress() {
    use std::io::{Read, Write};
    if !have_cc() {
        return;
    }
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let port = listener.local_addr().unwrap().port();
    let server = std::thread::spawn(move || {
        for _ in 0..4 {
            let mut slow = None;
            let mut fast = None;
            for _ in 0..2 {
                let mut stream = accept_tcp(&listener);
                let mut request = Vec::new();
                stream.read_to_end(&mut request).unwrap();
                if request == b"fast\0request" {
                    fast = Some(stream);
                } else {
                    assert_eq!(request.len(), 1048576);
                    assert!(request.iter().all(|b| *b == b's'));
                    slow = Some(stream);
                }
            }
            let mut fast = fast.unwrap();
            fast.write_all(b"f\0ast").unwrap();
            drop(fast);
            // The source must finish its fast await before it can open this gate.
            // A blocking read on the slow exchange would deadlock this protocol.
            let mut gate = accept_tcp(&listener);
            let mut request = Vec::new();
            gate.read_to_end(&mut request).unwrap();
            assert_eq!(request, b"release");
            let mut slow = slow.unwrap();
            slow.write_all(b"slow").unwrap();
            drop(slow);
            gate.write_all(b"gate").unwrap();
        }
    });
    let source = format!(
        r#"<?php
async function main(): void {{
    $unused = tcpExchangeAsync("invalid address", 0, "", -1); drop($unused);
    $slow = spawn tcpExchangeAsync("127.0.0.1", {port}, str_repeat("s", 1048576), 4);
    $fast = spawn tcpExchangeAsync("127.0.0.1", {port}, "fast\0request", 5);
    echo await $fast, ":";
    $gate = spawn tcpExchangeAsync("127.0.0.1", {port}, "release", 4);
    echo await $slow, ":", await $gate, "\n";
}}
"#
    );
    run_case("tcp-concurrent", &source, "f\0ast:slow:gate\n");
    server.join().unwrap();
}
#[test]
fn tcp_exchange_handles_ipv4_ipv6_and_empty_eof() {
    use std::io::Read;
    if !have_cc() {
        return;
    }
    for address in ["127.0.0.1", "::1"] {
        let listener = std::net::TcpListener::bind((address, 0)).unwrap();
        listener.set_nonblocking(true).unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = std::thread::spawn(move || {
            for _ in 0..4 {
                let mut stream = accept_tcp(&listener);
                let mut request = Vec::new();
                stream.read_to_end(&mut request).unwrap();
                assert!(request.is_empty());
                // Dropping sends EOF, including an empty response with maxBytes=0.
            }
        });
        let source = format!("<?php async function main(): void {{ echo strlen(await tcpExchangeAsync(\"{address}\", {port}, \"\", 0)), \"\\n\"; }}");
        run_case(
            if address == "::1" { "tcp-v6" } else { "tcp-v4" },
            &source,
            "0\n",
        );
        server.join().unwrap();
    }
}
#[test]
fn tcp_exchange_rejects_oversized_responses_and_invalid_addresses() {
    use std::io::{Read, Write};
    if !have_cc() {
        return;
    }
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let port = listener.local_addr().unwrap().port();
    let server = std::thread::spawn(move || {
        for _ in 0..4 {
            let mut stream = accept_tcp(&listener);
            let mut request = Vec::new();
            stream.read_to_end(&mut request).unwrap();
            // First receive can fill an 8192-byte private buffer; the extra byte
            // must fail the whole task, release that buffer, and close its fd.
            let _ = stream.write_all(&vec![b'x'; 8193]);
        }
    });
    for opt in [0, 3] {
        let source = format!("<?php async function main(): void {{ echo await tcpExchangeAsync(\"127.0.0.1\", {port}, \"\", 8192); }}");
        let executable = build("tcp-overflow", &source, opt);
        for stress in [false, true] {
            let mut command = Command::new(&executable);
            if stress {
                command.env("PXR_GC_STRESS", "1");
            }
            let out = execute(&mut command);
            assert_eq!(out.status.code(), Some(255));
            assert!(out.stdout.is_empty());
            let stderr = String::from_utf8_lossy(&out.stderr);
            assert!(stderr.starts_with("pxr: async task failed:"), "{stderr}");
            assert!(
                !stderr.contains("Sanitizer") && !stderr.contains("runtime error:"),
                "{stderr}"
            );
        }
        std::fs::remove_dir_all(executable.parent().unwrap()).unwrap();
    }
    server.join().unwrap();
    for timeout in [0, -1] {
        let source = format!("<?php async function main(): void {{ await tcpExchangeAsync(\"127.0.0.1\", 1, \"\", 1, {timeout}); }}");
        let executable = build(
            if timeout == 0 {
                "deadline-zero"
            } else {
                "deadline-negative"
            },
            &source,
            0,
        );
        let out = execute(Command::new(&executable).env("PXR_GC_STRESS", "1"));
        assert_eq!(out.status.code(), Some(255));
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(stderr.starts_with("pxr: async task failed:"), "{stderr}");
        assert!(
            !stderr.contains("Sanitizer") && !stderr.contains("runtime error:"),
            "{stderr}"
        );
        std::fs::remove_dir_all(executable.parent().unwrap()).unwrap();
    }
    for (name, address, port, limit) in [
        ("hostname", "localhost", 80, 10),
        ("nul-host", "127.0.0.1\\0bad", 80, 10),
        ("port-zero", "127.0.0.1", 0, 10),
        ("port-overflow", "127.0.0.1", 65536, 10),
        ("negative-limit", "127.0.0.1", 80, -1),
    ] {
        for opt in [0, 3] {
            let source = format!("<?php async function main(): void {{ await tcpExchangeAsync(\"{address}\", {port}, \"\", {limit}); }}");
            let executable = build(name, &source, opt);
            let out = execute(Command::new(&executable).env("PXR_GC_STRESS", "1"));
            assert_eq!(out.status.code(), Some(255), "{name}");
            let stderr = String::from_utf8_lossy(&out.stderr);
            assert!(
                stderr.starts_with("pxr: async task failed:"),
                "{name}: {stderr}"
            );
            assert!(
                !stderr.contains("Sanitizer") && !stderr.contains("runtime error:"),
                "{name}: {stderr}"
            );
            std::fs::remove_dir_all(executable.parent().unwrap()).unwrap();
        }
    }
}
#[test]
fn cancel_task_consumes_lazy_and_started_handles_without_detaching() {
    run_case(
        "cancel",
        r#"<?php
async function slow(): void { echo "started:"; await sleepAsync(10000000); echo "bad"; }
async function main(): void {
    $lazy = slow(); cancelTask($lazy);
    cancelTask(slow());
    $running = spawn slow(); await sleepAsync(0); cancelTask($running);
    $a = sleepAsync(0); $b = sleepAsync(0);
    await (true ? move($a) : move($b));
    echo "cancelled\n";
}
"#,
        "started:cancelled\n",
    );
}

#[test]
fn cancelling_tcp_waits_finishes_the_scope_before_the_server_closes() {
    use std::io::{Read, Write};
    if !have_cc() {
        return;
    }
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let port = listener.local_addr().unwrap().port();
    let (done, finished) = std::sync::mpsc::channel();
    let server = std::thread::spawn(move || {
        let mut held = Vec::new();
        for _ in 0..4 {
            let mut hold = None;
            let mut gate = None;
            for _ in 0..2 {
                let mut stream = accept_tcp(&listener);
                let mut request = Vec::new();
                stream.read_to_end(&mut request).unwrap();
                match request.as_slice() {
                    b"hold" => hold = Some(stream),
                    b"gate" => gate = Some(stream),
                    other => panic!("unexpected gate protocol: {other:?}"),
                }
            }
            let mut gate = gate.unwrap();
            gate.write_all(b"ready").unwrap();
            drop(gate);
            held.push(hold.unwrap()); // Keep response streams open until every client exits.
        }
        finished.recv_timeout(Duration::from_secs(15)).unwrap();
        drop(held);
    });
    let source = format!(
        r#"<?php
async function pending(): void {{
    await tcpExchangeAsync("127.0.0.1", {port}, "hold", 100, 10000);
    echo "bad";
}}
async function main(): void {{
    $pending = spawn pending();
    echo await tcpExchangeAsync("127.0.0.1", {port}, "gate", 5), ":";
    cancelTask($pending);
    echo "cancelled\n";
}}
"#
    );
    run_case("cancel-tcp", &source, "ready:cancelled\n");
    done.send(()).unwrap();
    server.join().unwrap();
}

// Reserve only while choosing a port; the PXR process itself owns the listener.
fn server_port(address: &str) -> u16 {
    std::net::TcpListener::bind((address, 0))
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

#[test]
fn native_server_runs_suspended_handlers_concurrently() {
    use std::io::{Read, Write};
    if !have_cc() {
        return;
    }
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let gate_port = listener.local_addr().unwrap().port();
    let gate = std::thread::spawn(move || {
        for _ in 0..4 {
            let mut hold = None;
            let mut ready = None;
            for _ in 0..2 {
                let mut stream = accept_tcp(&listener);
                let mut request = Vec::new();
                stream.read_to_end(&mut request).unwrap();
                match request.as_slice() {
                    b"hold" => hold = Some(stream),
                    b"ready" => ready = Some(stream),
                    other => panic!("unexpected gate request: {other:?}"),
                }
            }
            // Only now may main send the fast request to the PXR listener.
            ready.take().unwrap().write_all(b"ok").unwrap();
            let mut release = accept_tcp(&listener);
            let mut request = Vec::new();
            release.read_to_end(&mut request).unwrap();
            assert_eq!(request, b"release");
            hold.take().unwrap().write_all(b"unblocked").unwrap();
            release.write_all(b"ok").unwrap();
        }
    });
    let port = server_port("127.0.0.1");
    let source = format!(
        r#"<?php
async function reply(owned<string> $request): string {{
    if ($request === "slow") {{
        return await tcpExchangeAsync("127.0.0.1", {gate_port}, "hold", 9);
    }}
    return "reply: " . $request;
}}
async function main(): void {{
    $server = spawn tcpServeAsync("127.0.0.1", {port}, "reply", 2, 4096, 10000, 2);
    await sleepAsync(0);
    $slow = spawn tcpExchangeAsync("127.0.0.1", {port}, "slow", 9);
    $ready = await tcpExchangeAsync("127.0.0.1", {gate_port}, "ready", 2);
    echo await tcpExchangeAsync("127.0.0.1", {port}, "fast\0", 12), "\n";
    $released = await tcpExchangeAsync("127.0.0.1", {gate_port}, "release", 2);
    echo await $slow, "\n";
    await $server;
}}
"#
    );
    run_case(
        "native-server-progress",
        &source,
        "reply: fast\0\nunblocked\n",
    );
    gate.join().unwrap();
}

#[test]
fn native_server_bounds_concurrency_and_resolves_namespaced_handlers() {
    for address in ["127.0.0.1", "::1"] {
        let port = server_port(address);
        let source = format!(
            r#"<?php
namespace App {{
use function tcpServeAsync as serve;
async function reply(string $request): string {{
    echo "start:";
    await sleepAsync(0);
    echo "end:";
    return str_repeat($request, 8192);
}}
async function serveThree(): void {{
    await serve("{address}", {port}, "App\\REPLY", 3, 8192, 10000, 1);
}}
}}
namespace {{
async function main(): void {{
    $server = spawn \App\serveThree();
    await sleepAsync(0);
    $a = spawn tcpExchangeAsync("{address}", {port}, "a", 8192);
    $b = spawn tcpExchangeAsync("{address}", {port}, "b", 8192);
    $c = spawn tcpExchangeAsync("{address}", {port}, "c", 8192);
    $x = await $a; $y = await $b; $z = await $c;
    echo strlen($x), ":", strlen($y), ":", strlen($z), "\n";
    await $server;
}}
}}
"#
        );
        // A second handler cannot start while the first crosses its await.
        run_case(
            &format!("native-server-serial-{}", port),
            &source,
            "start:end:start:end:start:end:8192:8192:8192\n",
        );
    }
}

#[test]
fn native_server_cancellation_drains_handlers_and_releases_listener() {
    if !have_cc() {
        return;
    }
    let port = server_port("127.0.0.1");
    let source = format!(
        r#"<?php
async function reply(string $request): string {{
    if ($request === "gate") {{ return "ready"; }}
    echo "handler:";
    await sleepAsync(10000000);
    return "bad";
}}
async function session(): void {{
    $lazy = tcpServeAsync("bad address", 0, "reply", -1, -1);
    cancelTask($lazy);
    $server = spawn tcpServeAsync("127.0.0.1", {port}, "reply", 0, 4096);
    await sleepAsync(0);
    $client = spawn tcpExchangeAsync("127.0.0.1", {port}, "request", 10);
    echo await tcpExchangeAsync("127.0.0.1", {port}, "gate", 5), ":";
    cancelTask($client);
    cancelTask($server);
    echo "cancelled\n";
}}
async function main(): void {{
    await session(); // Scope exit joins cancellation before rebinding the port.
    $server = spawn tcpServeAsync("127.0.0.1", {port}, "reply", 1, 4096);
    await sleepAsync(0);
    echo await tcpExchangeAsync("127.0.0.1", {port}, "gate", 5), "\n";
    await $server;
}}
"#
    );
    run_case(
        "native-server-cancel",
        &source,
        "handler:ready:cancelled\nready\n",
    );
    // Process teardown alone cannot prove listener cleanup; the C harness also
    // cancels/rebinds inside one process and checks accepted descriptor reuse.
}

#[test]
fn native_server_rejects_invalid_handlers_and_call_shapes() {
    let dir = std::env::temp_dir().join(format!("pxr-m7-server-errors-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let cases = [
        "function tcpServeAsync(): void {}",
        "namespace X; function TCPServeAsync(): void {}",
        "async function main(): void { await tcpServeAsync(\"127.0.0.1\", 80, \"missing\", 1, 10); }",
        "async function reply(int $r): string { return \"\"; } async function main(): void { await tcpServeAsync(\"127.0.0.1\", 80, \"reply\", 1, 10); }",
        "async function reply(string $r): int { return 1; } async function main(): void { await tcpServeAsync(\"127.0.0.1\", 80, \"reply\", 1, 10); }",
        "function reply(string $r): string { return $r; } async function main(): void { await tcpServeAsync(\"127.0.0.1\", 80, \"reply\", 1, 10); }",
        "async function reply(string $r): string { return $r; } function f(): void { tcpServeAsync(\"127.0.0.1\", 80, \"reply\", 1, 10); }",
        "async function reply(string $r): string { return $r; } async function main(): void { $name = \"reply\"; await tcpServeAsync(\"127.0.0.1\", 80, $name, 1, 10); }",
        "async function reply(string $r): string { return $r; } async function main(): void { await tcpServeAsync(1, 80, \"reply\", 1, 10); }",
        "async function reply(string $r): string { return $r; } async function main(): void { await tcpServeAsync(\"127.0.0.1\", 80, \"reply\", 1, 10, \"bad\"); }",
        "async function reply(string $r): string { return $r; } async function main(): void { await tcpServeAsync(\"127.0.0.1\", 80, \"reply\", 1, 10, 1000, true); }",
        "async function reply(string $r): string { return $r; } async function main(): void { await tcpServeAsync(\"127.0.0.1\", 80, \"reply\", 1); }",
        "async function reply(string $r): string { return $r; } async function main(): void { await tcpServeAsync<int>(\"127.0.0.1\", 80, \"reply\", 1, 10); }",
        "async function reply(string $r): string { return $r; } async function main(): void { $s = spawn tcpServeAsync(\"127.0.0.1\", 80, \"reply\", 1, 10); cancelTask($s); await $s; }",
    ];
    for (i, source) in cases.iter().enumerate() {
        let input = dir.join(format!("invalid-{i}.php"));
        std::fs::write(&input, format!("<?php {source}")).unwrap();
        let out = Command::new(env!("CARGO_BIN_EXE_pxr"))
            .arg("check")
            .arg(input)
            .output()
            .unwrap();
        assert!(!out.status.success(), "case {i} unexpectedly accepted");
        let error = String::from_utf8_lossy(&out.stderr);
        assert!(
            error.contains("error") && !error.contains("panicked"),
            "case {i}: {error}"
        );
    }
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn native_server_empty_requests_and_repeated_listener_cleanup() {
    let port = server_port("127.0.0.1");
    run_case(
        "native-server-empty",
        &format!(
            r#"<?php
async function reply(string $request): string {{ return $request; }}
async function cancelledListener(): void {{
    $s = spawn tcpServeAsync("127.0.0.1", {port}, "reply", 0, 0);
    await sleepAsync(0);
    cancelTask($s);
}}
async function main(): void {{
    for ($i = 0; $i < 100; $i++) {{
        await cancelledListener();
        $s = spawn tcpServeAsync("127.0.0.1", {port}, "reply", 1, 0, 30000);
        await sleepAsync(0);
        $empty = await tcpExchangeAsync("127.0.0.1", {port}, "", 0);
        if (strlen($empty) !== 0) {{ echo "bad"; }}
        await $s;
    }}
    echo "clean\n";
}}
"#
        ),
        "clean\n",
    );
}

#[test]
fn native_server_limits_deadlines_and_bind_errors_fail_after_draining() {
    if !have_cc() {
        return;
    }
    let occupied = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let occupied_port = occupied.local_addr().unwrap().port();
    let port = server_port("127.0.0.1");
    let main = |handler: &str, bytes: i32, timeout: i32, request: &str| {
        format!(
            r#"<?php
{handler}
async function main(): void {{
    $server = spawn tcpServeAsync("127.0.0.1", {port}, "reply", 1, {bytes}, {timeout});
    await sleepAsync(0);
    await tcpExchangeAsync("127.0.0.1", {port}, "{request}", 100);
    await $server;
    echo "bad";
}}
"#
        )
    };
    let echo = "async function reply(string $r): string { return $r; }";
    let mut cases = vec![
        main(echo, 1, 30000, "ab"),
        main(
            "async function reply(string $r): string { return \"ab\"; }",
            1,
            30000,
            "",
        ),
        main(
            "async function reply(string $r): string { await sleepAsync(10000000); return $r; }",
            10,
            10,
            "",
        ),
        main(
            "async function reply(string $r): string { await sleepAsync(-1); return $r; }",
            10,
            30000,
            "",
        ),
    ];
    for (address, port, requests, bytes, timeout, concurrent) in [
        ("localhost", port, 1, 10, 1000, 1),
        ("127.0.0.1\\0", port, 1, 10, 1000, 1),
        ("127.0.0.1", 0, 1, 10, 1000, 1),
        ("127.0.0.1", port, -1, 10, 1000, 1),
        ("127.0.0.1", port, 1, -1, 1000, 1),
        ("127.0.0.1", port, 1, 10, 0, 1),
        ("127.0.0.1", port, 1, 10, -1, 1),
        ("127.0.0.1", port, 1, 10, 1000, 0),
        ("127.0.0.1", port, 1, 10, 1000, 1025),
        ("127.0.0.1", occupied_port, 1, 10, 1000, 1),
    ] {
        cases.push(format!("<?php {echo} async function main(): void {{ await tcpServeAsync(\"{address}\", {port}, \"reply\", {requests}, {bytes}, {timeout}, {concurrent}); echo \"bad\"; }}"));
    }
    for (i, source) in cases.iter().enumerate() {
        for opt in [0, 3] {
            let executable = build(&format!("native-server-failure-{i}"), source, opt);
            for stress in [false, true] {
                let mut command = Command::new(&executable);
                command.env("PXR_PHP", "/pxr-no-php-cli");
                if stress {
                    command.env("PXR_GC_STRESS", "1");
                }
                let out = execute(&mut command);
                assert_eq!(out.status.code(), Some(255), "case {i}: {out:?}");
                assert!(out.stdout.is_empty(), "case {i}: {out:?}");
                let error = String::from_utf8_lossy(&out.stderr);
                assert!(
                    error.starts_with("pxr: async task failed:"),
                    "case {i}: {error}"
                );
            }
            std::fs::remove_dir_all(executable.parent().unwrap()).unwrap();
        }
    }
}

#[test]
fn native_server_round_trips_large_binary_requests() {
    let port = server_port("127.0.0.1");
    run_case(
        "native-server-large",
        &format!(
            r#"<?php
async function reply(string $request): string {{
    await sleepAsync(0);
    return $request;
}}
async function main(): void {{
    $s = spawn tcpServeAsync("127.0.0.1", {port}, "reply", 8, 1048576);
    await sleepAsync(0);
    for ($i = 0; $i < 8; $i++) {{
        $reply = await tcpExchangeAsync("127.0.0.1", {port}, str_repeat("x\0", 524288), 1048576);
        if (strlen($reply) !== 1048576 || substr($reply, 0, 4) !== "x\0x\0" || substr($reply, 1048572, 4) !== "x\0x\0") {{ echo "bad"; }}
    }}
    await $s;
    echo "binary clean\n";
}}
"#
        ),
        "binary clean\n",
    );
}

fn connect_http(port: u16) -> std::net::TcpStream {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        match std::net::TcpStream::connect(("127.0.0.1", port)) {
            Ok(stream) => {
                stream
                    .set_read_timeout(Some(Duration::from_secs(10)))
                    .unwrap();
                stream
                    .set_write_timeout(Some(Duration::from_secs(10)))
                    .unwrap();
                stream.set_nodelay(true).unwrap();
                return stream;
            }
            Err(error)
                if error.kind() == std::io::ErrorKind::ConnectionRefused
                    && Instant::now() < deadline =>
            {
                std::thread::sleep(Duration::from_millis(5))
            }
            Err(error) => panic!("HTTP connect: {error}"),
        }
    }
}
fn read_http(stream: &mut std::net::TcpStream) -> (String, Vec<u8>) {
    use std::io::Read;
    let mut response = Vec::new();
    stream.read_to_end(&mut response).unwrap();
    while response.starts_with(b"HTTP/1.1 100 Continue\r\n\r\n") {
        response.drain(..25);
    }
    let split = response
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .expect("HTTP response headers");
    let headers = String::from_utf8(response[..split].to_vec()).unwrap();
    assert!(headers.contains("Connection: close\r\n"));
    (headers, response[split + 4..].to_vec())
}
fn http_cases(
    name: &str,
    handler: &str,
    requests: Vec<(Vec<u8>, u16, Vec<u8>)>,
    limit: i64,
) -> Vec<Vec<String>> {
    use std::io::Write;
    if !have_cc() {
        return vec![];
    }
    let port = server_port("127.0.0.1");
    let count = requests.len();
    let source = format!("<?php {handler}\nasync function main(): void {{ await httpServeAsync(\"127.0.0.1\", {port}, \"handle\", {count}, {limit}, 10000); echo \"done\\n\"; }}");
    let mut all_headers = Vec::new();
    for opt in [0, 3] {
        let executable = build(name, &source, opt);
        for stress in [false, true] {
            let requests = requests.clone();
            let client = std::thread::spawn(move || {
                let mut results = Vec::new();
                for (request, status, body) in requests {
                    let mut stream = connect_http(port);
                    let wait_continue =
                        status < 400 && request.windows(20).any(|w| w == b"Expect: 100-continue");
                    let request = if wait_continue {
                        use std::io::Read;
                        let split = request.windows(4).position(|w| w == b"\r\n\r\n").unwrap() + 4;
                        stream.write_all(&request[..split]).unwrap();
                        let mut interim = [0; 25];
                        stream.read_exact(&mut interim).unwrap();
                        assert_eq!(&interim, b"HTTP/1.1 100 Continue\r\n\r\n");
                        request[split..].to_vec()
                    } else {
                        request
                    };
                    for chunk in request.chunks(if request.len() > 65536 { 8192 } else { 7 }) {
                        stream.write_all(chunk).unwrap();
                    }
                    // Leave the write side open, as a browser does. The response
                    // must be framed by headers/Content-Length, never TCP EOF.
                    let (headers, actual) = read_http(&mut stream);
                    assert!(
                        headers.starts_with(&format!("HTTP/1.1 {status} ")),
                        "{headers}"
                    );
                    assert_eq!(actual, body, "{headers}");
                    results.push(headers);
                }
                results
            });
            let mut command = Command::new(&executable);
            command.env("PXR_PHP", "/pxr-no-php-cli");
            if stress {
                command.env("PXR_GC_STRESS", "1");
            }
            let output = execute(&mut command);
            assert!(
                output.status.success(),
                "{name}: {}",
                String::from_utf8_lossy(&output.stderr)
            );
            assert_eq!(output.stdout, b"done\n");
            assert!(
                output.stderr.is_empty(),
                "{name}: {}",
                String::from_utf8_lossy(&output.stderr)
            );
            all_headers.push(client.join().unwrap());
        }
        std::fs::remove_dir_all(executable.parent().unwrap()).unwrap();
    }
    all_headers
}
#[test]
fn http_frames_requests_and_exposes_php_request_globals() {
    let handler = r#"
function currentName(): string { return strval($_GET["name"] ?? "none"); }
async function afterAwait(): string { await sleepAsync(0); return currentName(); }
async function handle(): string {
    header("Content-Type: text/plain");
    header("X-Version: old"); header("X-Version: new");
    header("Set-Cookie: a=1", false); header("Set-Cookie: b=2", false);
    http_response_code(201);
    $saved = $_GET;
    $_GET["name"] = "local";
    $name = await afterAwait();
    return strval($_SERVER["REQUEST_METHOD"]) . ":" . strval($saved["name"] ?? "none") . ":" . $name . ":" . strval($_POST["name"] ?? "none") . ":" . strval($_REQUEST["name"] ?? "none") . ":" . strval($_COOKIE["id"] ?? "none") . ":" . httpRequestBody();
}
"#;
    let headers = http_cases("http-globals", handler, vec![
        (b"GET /hello?name=Ada+Lovelace HTTP/1.1\r\nHost: localhost\r\nCookie: id=a+b; id=second\r\n\r\n".to_vec(), 201, b"GET:Ada Lovelace:local:none:Ada Lovelace:a+b:".to_vec()),
        (b"POST /hello?name=query HTTP/1.1\r\nHost: localhost\r\nContent-Type: application/x-www-form-urlencoded\r\nContent-Length: 9\r\n\r\nname=form".to_vec(), 201, b"POST:query:local:form:form:none:name=form".to_vec()),
        (b"PUT /bytes HTTP/1.0\r\nContent-Length: 3\r\n\r\na\0b".to_vec(), 201, b"PUT:none:local:none:none:none:a\0b".to_vec()),
    ], 4096);
    for run in headers {
        for headers in run {
            assert!(headers.contains("Content-Type: text/plain\r\n"));
            assert!(!headers.contains("X-Version: old"));
            assert_eq!(headers.matches("X-Version: new").count(), 1);
            assert_eq!(headers.matches("Set-Cookie:").count(), 2);
        }
    }
}
#[test]
fn http_head_statuses_and_protocol_errors_are_connection_local() {
    let handler = r#"
async function handle(): string {
    $path = strval($_SERVER["PATH_INFO"]);
    if ($path === "/empty") { http_response_code(204); }
    if ($path === "/redirect") { header("Location: /target"); }
    if ($path === "/invalid") { header("X-Test: yes\r\nInjected: bad"); }
    if ($path === "/missing") { header("http/1.1 404 Not Found"); }
    return "hello";
}
"#;
    let mut cases = vec![
        (
            b"HEAD / HTTP/1.1\r\nHost: localhost\r\n\r\n".to_vec(),
            200,
            vec![],
        ),
        (
            b"GET /empty HTTP/1.1\r\nHost: localhost\r\n\r\n".to_vec(),
            204,
            vec![],
        ),
        (
            b"GET /redirect HTTP/1.1\r\nHost: localhost\r\n\r\n".to_vec(),
            302,
            b"hello".to_vec(),
        ),
        (
            b"GET /missing HTTP/1.1\r\nHost: localhost\r\n\r\n".to_vec(),
            404,
            b"hello".to_vec(),
        ),
        (
            b"GET /invalid HTTP/1.1\r\nHost: localhost\r\n\r\n".to_vec(),
            500,
            b"Internal Server Error\n".to_vec(),
        ),
    ];
    for request in [
        "GET / HTTP/1.1\r\n\r\n", // missing Host
        "GET / HTTP/1.1\r\nHost: one\r\nHost: two\r\n\r\n",
        "GET / HTTP/1.1\r\nHost: localhost\r\nContent-Length: 0\r\nContent-Length: 0\r\n\r\n",
        "GET / HTTP/1.1\r\nHost: localhost\r\nBad : value\r\n\r\n",
        "GET / HTTP/1.1\r\nHost: localhost\r\n Folded: value\r\n\r\n",
        "GET / HTTP/1.1\r\nHost: localhost\r\nTransfer-Encoding: chunked\r\nContent-Length: 0\r\n\r\n",
    ] { cases.push((request.as_bytes().to_vec(), 400, b"Bad Request\n".to_vec())); }
    cases.extend([
        (b"POST / HTTP/1.1\r\nHost: localhost\r\nTransfer-Encoding: gzip, chunked\r\n\r\n".to_vec(), 501, b"Not Implemented\n".to_vec()),
        (b"POST / HTTP/1.1\r\nHost: localhost\r\nExpect: something-else\r\nContent-Length: 3\r\n\r\n".to_vec(), 417, b"Expectation Failed\n".to_vec()),
        (b"POST / HTTP/1.1\r\nHost: localhost\r\nContent-Type: multipart/form-data; boundary=x\r\n\r\n".to_vec(), 415, b"Unsupported Media Type\n".to_vec()),
        (b"POST / HTTP/1.1\r\nHost: localhost\r\nContent-Length: 4097\r\n\r\n".to_vec(), 413, b"Content Too Large\n".to_vec()),
        (b"GET / HTTP/2.0\r\nHost: localhost\r\n\r\n".to_vec(), 505, b"HTTP Version Not Supported\n".to_vec()),
        (b"GET / HTTP/1.1\r\nHost: localhost\r\n\r\n".to_vec(), 200, b"hello".to_vec()),
    ]);
    http_cases("http-protocol", handler, cases, 4096);
}

#[test]
fn http_php_output_and_nested_parameters_survive_await() {
    let handler = r#"
async function handle(): void {
    header("Content-Type: text/plain");
    $tags = $_GET["tags"] ?? [];
    if (is_array($tags)) { echo $tags[0], ":", $tags[1], ":"; }
    $form = $_POST["user"] ?? [];
    if (is_array($form)) { echo $form["name"], ":"; }
    echo $_GET["a_b"] ?? "none", ":";
    echo http_response_code(), ":";
    await sleepAsync(0);
    echo 42, ":", 1.5, ":", true, ":";
    print "print:";
    printf("%s:%d", "printf", 7);
}
"#;
    http_cases("http-php-output", handler, vec![(
        b"POST /?tags[]=one&tags[]=two&a.b=last HTTP/1.1\r\nHost: localhost\r\nContent-Type: application/x-www-form-urlencoded\r\nContent-Length: 19\r\n\r\nuser[name]=Ada+Love".to_vec(),
        200, b"one:two:Ada Love:last:200:42:1.5:1:print:printf:7".to_vec(),
    )], 4096);
}
#[test]
fn http_request_contexts_are_isolated_across_concurrent_handlers_and_children() {
    use std::io::{Read, Write};
    if !have_cc() {
        return;
    }
    let gate = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    gate.set_nonblocking(true).unwrap();
    let gate_port = gate.local_addr().unwrap().port();
    let port = server_port("127.0.0.1");
    let source = format!(
        r#"<?php
function currentId(): string {{ return strval($_GET["id"]); }}
async function child(): string {{ await sleepAsync(0); echo "child:"; return currentId(); }}
async function handle(): string {{
    $id = currentId();
    $_GET["id"] = $id . "-changed";
    $_SERVER["MARK"] = $id;
    header("X-Request-ID: " . $id);
    if ($id === "slow") {{
        http_response_code(202);
        await tcpExchangeAsync("127.0.0.1", {gate_port}, "hold", 2);
    }} else {{ http_response_code(201); }}
    $value = await child();
    return $id . ":" . $value . ":" . strval($_REQUEST["id"]) . ":" . strval($_SERVER["MARK"]);
}}
async function main(): void {{
    $_GET["id"] = "cli";
    await httpServeAsync("127.0.0.1", {port}, "handle", 2, 4096, 10000, 2);
    echo currentId(), "\n";
}}
"#
    );
    for opt in [0, 3] {
        let executable = build("http-context-isolation", &source, opt);
        for stress in [false, true] {
            let listener = gate.try_clone().unwrap();
            let client = std::thread::spawn(move || {
                let mut slow = connect_http(port);
                slow.write_all(b"GET /?id=slow HTTP/1.1\r\nHost: localhost\r\n\r\n")
                    .unwrap();
                let mut hold = accept_tcp(&listener);
                let mut token = Vec::new();
                hold.read_to_end(&mut token).unwrap();
                assert_eq!(token, b"hold"); // Slow handler is suspended before fast starts.
                let mut fast = connect_http(port);
                fast.write_all(b"GET /?id=fast HTTP/1.1\r\nHost: localhost\r\n\r\n")
                    .unwrap();
                let (headers, body) = read_http(&mut fast);
                assert!(
                    headers.starts_with("HTTP/1.1 201 ") && headers.contains("X-Request-ID: fast")
                );
                assert_eq!(body, b"child:fast:fast-changed:fast:fast");
                hold.write_all(b"ok").unwrap();
                drop(hold);
                let (headers, body) = read_http(&mut slow);
                assert!(
                    headers.starts_with("HTTP/1.1 202 ") && headers.contains("X-Request-ID: slow")
                );
                assert!(!headers.contains("X-Request-ID: fast"));
                assert_eq!(body, b"child:slow:slow-changed:slow:slow");
            });
            let mut command = Command::new(&executable);
            command.env("PXR_PHP", "/pxr-no-php-cli");
            if stress {
                command.env("PXR_GC_STRESS", "1");
            }
            let result = execute(&mut command);
            assert!(
                result.status.success(),
                "{}",
                String::from_utf8_lossy(&result.stderr)
            );
            assert_eq!(result.stdout, b"cli\n");
            assert!(result.stderr.is_empty());
            client.join().unwrap();
        }
        std::fs::remove_dir_all(executable.parent().unwrap()).unwrap();
    }
}
#[test]
fn php_request_globals_work_in_sync_helpers_and_reject_unsupported_forms() {
    run_case(
        "http-cli-globals",
        r#"<?php
function name(): string { return strval($_GET["name"] ?? "none"); }
$_GET["name"] = "CLI";
echo name(), ":", count($_POST), ":", count($_FILES), "\n";
"#,
        "CLI:0:0\n",
    );
    let dir = std::env::temp_dir().join(format!("pxr-http-errors-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    for (i, source) in [
        "async function handle(string $request): string { return $request; } async function main(): void { await httpServeAsync(\"127.0.0.1\", 8080, \"handle\", 1, 10); }",
        "function handle(): string { return \"x\"; } async function main(): void { await httpServeAsync(\"127.0.0.1\", 8080, \"handle\", 1, 10); }",
        "async function handle(): int { return 1; } async function main(): void { await httpServeAsync(\"127.0.0.1\", 8080, \"handle\", 1, 10); }",
        "function httpServeAsync(): void {}",
        "namespace X; function header(string $line): void {}",
        "function f(array<mixed> $_GET): void {}",
        "class X { public function f(array<mixed> $_POST): void {} }",
        "$_GET = [];", "$_POST: owned<array<int>> = [];", "$_SESSION = [];",
        "echo $_SESSION;", "echo $_ENV;", "echo $GLOBALS;",
        "foreach ([1] as $_GET) {}",
        "header(1);", "header(\"X: y\", 1);", "header(\"X: y\", true, \"200\");",
        "http_response_code(\"200\");", "httpRequestBody(1);",
        "async function f(): int { return 1; } async function main(): void { $_GET[\"task\"] = f(); }",
    ].iter().enumerate() {
        let input = dir.join(format!("{i}.php")); std::fs::write(&input, format!("<?php {source}")).unwrap();
        let output = Command::new(env!("CARGO_BIN_EXE_pxr")).arg("check").arg(input).output().unwrap();
        assert!(!output.status.success(), "case {i} unexpectedly accepted");
        let error = String::from_utf8_lossy(&output.stderr);
        assert!(error.contains("error") && !error.contains("panicked"), "{i}: {error}");
    }
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn http_bounds_headers_and_echoed_response_content() {
    let mut oversized = b"GET / HTTP/1.1\r\nHost: localhost\r\nX-Large: ".to_vec();
    oversized.resize(oversized.len() + 17000, b'x');
    oversized.extend_from_slice(b"\r\n\r\n");
    let mut many = b"GET / HTTP/1.1\r\nHost: localhost\r\n".to_vec();
    for i in 0..100 {
        many.extend_from_slice(format!("X-{i}: y\r\n").as_bytes());
    }
    many.extend_from_slice(b"\r\n");
    http_cases(
        "http-bounds",
        "async function handle(): void { echo \"abc\"; }",
        vec![
            (
                oversized,
                431,
                b"Request Header Fields Too Large\n".to_vec(),
            ),
            (many, 431, b"Request Header Fields Too Large\n".to_vec()),
            (
                b"GET / HTTP/1.1\r\nHost: localhost\r\n\r\n".to_vec(),
                500,
                b"Internal Server Error\n".to_vec(),
            ),
        ],
        2,
    );
}
#[test]
fn http_round_trips_large_binary_bodies_without_client_eof() {
    let mut request = b"POST /bytes HTTP/1.1\r\nHost: localhost\r\nContent-Type: application/octet-stream\r\nContent-Length: 1048576\r\n\r\n".to_vec();
    let body: Vec<u8> = (0..1048576).map(|i| (i % 256) as u8).collect();
    request.extend_from_slice(&body);
    let mut chunked =
        b"POST /bytes HTTP/1.1\r\nHost: localhost\r\nTransfer-Encoding: chunked\r\n\r\n".to_vec();
    for chunk in body.chunks(16381) {
        chunked.extend_from_slice(format!("{:x};binary\r\n", chunk.len()).as_bytes());
        chunked.extend_from_slice(chunk);
        chunked.extend_from_slice(b"\r\n");
    }
    chunked.extend_from_slice(b"0\r\nX-Checksum: ignored\r\n\r\n");
    http_cases("http-binary", "async function handle(): string { await sleepAsync(0); header(\"Content-Type: application/octet-stream\"); return httpRequestBody(); }", vec![(request, 200, body.clone()), (chunked, 200, body)], 1048576);
}

#[test]
fn http_decodes_chunked_forms_and_handles_continue_handshakes() {
    http_cases(
        "http-chunked-form",
        r#"async function handle(): string {
            await sleepAsync(0);
            echo $_POST["name"], ":", $_REQUEST["name"], ":";
            return httpRequestBody();
        }"#,
        vec![
            (b"POST /?name=query HTTP/1.1\r\nHost: localhost\r\nTransfer-Encoding: ChUnKeD\r\nContent-Type: application/x-www-form-urlencoded\r\n\r\n4;foo=\"a\\\"b\"\r\nname\r\n5\r\n=form\r\n0\r\nX-Checksum: ignored\r\n\r\n".to_vec(), 200, b"form:form:name=form".to_vec()),
            (b"POST / HTTP/1.1\r\nHost: localhost\r\nExpect: 100-continue\r\nContent-Type: application/x-www-form-urlencoded\r\nContent-Length: 9\r\n\r\nname=form".to_vec(), 200, b"form:form:name=form".to_vec()),
            (b"POST / HTTP/1.1\r\nHost: localhost\r\nExpect: 100-continue\r\nContent-Type: application/x-www-form-urlencoded\r\nTransfer-Encoding: chunked\r\n\r\n9\r\nname=form\r\n0\r\n\r\n".to_vec(), 200, b"form:form:name=form".to_vec()),
        ],
        4096,
    );
}

#[test]
fn http_rejects_ambiguous_chunks_and_recovers_for_the_next_request() {
    let mut requests = Vec::new();
    for (fields, wire, status, reason) in [
        (
            "Transfer-Encoding: chunked",
            "1\r\naX\n",
            400,
            "Bad Request\n",
        ),
        (
            "Transfer-Encoding: chunked",
            "0\r\nContent-Type: forged\r\n\r\n",
            400,
            "Bad Request\n",
        ),
        (
            "Transfer-Encoding: chunked",
            "0\r\nCookie: evil=1\r\n\r\n",
            400,
            "Bad Request\n",
        ),
        (
            "Transfer-Encoding: chunked, chunked",
            "",
            400,
            "Bad Request\n",
        ),
        ("Transfer-Encoding: chunked, gzip", "", 400, "Bad Request\n"),
        (
            "Transfer-Encoding: gzip, chunked",
            "",
            501,
            "Not Implemented\n",
        ),
        ("Transfer-Encoding: chunked,", "", 400, "Bad Request\n"),
        (
            "Transfer-Encoding: chunked\r\nContent-Length: 0",
            "",
            400,
            "Bad Request\n",
        ),
        (
            "Transfer-Encoding: chunked",
            "1001\r\n",
            413,
            "Content Too Large\n",
        ),
        (
            "Expect: 100-continue\r\nContent-Length: 4097",
            "",
            413,
            "Content Too Large\n",
        ),
    ] {
        requests.push((
            format!("POST / HTTP/1.1\r\nHost: localhost\r\n{fields}\r\n\r\n{wire}").into_bytes(),
            status,
            reason.as_bytes().to_vec(),
        ));
    }
    requests.push((
        b"POST / HTTP/1.0\r\nTransfer-Encoding: chunked\r\n\r\n".to_vec(),
        400,
        b"Bad Request\n".to_vec(),
    ));
    requests.push((
        b"GET / HTTP/1.1\r\nHost: localhost\r\n\r\n".to_vec(),
        200,
        b"ok".to_vec(),
    ));
    http_cases(
        "http-chunked-errors",
        "async function handle(): string { return \"ok\"; }",
        requests,
        4096,
    );
}

#[test]
fn http_handler_failures_join_descendants_and_return_clean_500_responses() {
    let gate = server_port("127.0.0.1");
    let handler = format!(
        r#"
async function reply(string $request): string {{ return $request; }}
async function failChild<T>(T $context): void {{ await sleepAsync(-1); }}
async function handle(): string {{
    if ($_SERVER["PATH_INFO"] === "/direct") {{
        header("X-Private: secret"); echo "private";
        await failChild("direct");
        return "unreachable";
    }}
    $listener = spawn tcpServeAsync("127.0.0.1", {gate}, "reply", 0, 10);
    await sleepAsync(0);
    if ($_SERVER["PATH_INFO"] === "/tree") {{
        header("X-Private: secret"); http_response_code(201); echo "private";
        spawn failChild<int>(1);
        return "discarded";
    }}
    cancelTask($listener);
    return "ok";
}}"#
    );
    let requests = [
        ("/tree", 500),
        ("/healthy", 200),
        ("/direct", 500),
        ("/healthy", 200),
    ]
    .into_iter()
    .map(|(path, status)| {
        (
            format!("GET {path} HTTP/1.1\r\nHost: localhost\r\n\r\n").into_bytes(),
            status,
            if status == 500 {
                b"Internal Server Error\n".to_vec()
            } else {
                b"ok".to_vec()
            },
        )
    })
    .collect();
    for run in http_cases("http-handler-recovery", &handler, requests, 4096) {
        for headers in run {
            assert!(!headers.contains("X-Private"));
        }
    }
}

#[test]
fn http_deadlines_release_connection_slots_and_cancel_handler_resources() {
    use std::io::{Read, Write};
    if !have_cc() {
        return;
    }
    let port = server_port("127.0.0.1");
    let gate = server_port("127.0.0.1");
    let source = format!(
        r#"<?php
async function reply(string $request): string {{ return $request; }}
async function handle(): string {{
    $listener = spawn tcpServeAsync("127.0.0.1", {gate}, "reply", 0, 10);
    await sleepAsync(0);
    if ($_SERVER["PATH_INFO"] === "/slow") {{
        echo "private"; await sleepAsync(10000000);
    }}
    cancelTask($listener); return "ok";
}}
async function main(): void {{
    await httpServeAsync("127.0.0.1", {port}, "handle", 6, 4096, 500, 1);
    echo "done\n";
}}"#
    );
    for opt in [0, 3] {
        let executable = build("http-deadline-recovery", &source, opt);
        for stress in [false, true] {
            let client = std::thread::spawn(move || {
                for partial in [
                    b"GET / HTTP/1.1\r\nHost: localhost\r\n".as_slice(),
                    b"POST / HTTP/1.1\r\nHost: localhost\r\nTransfer-Encoding: chunked\r\n\r\n8\r\nabc",
                    b"GET /slow HTTP/1.1\r\nHost: localhost\r\n\r\n",
                ] {
                    let mut bad = connect_http(port);
                    bad.write_all(partial).unwrap();
                    // Queue work at capacity. It must run after timeout frees the
                    // sole slot, with the failed handler's listener already closed.
                    let mut healthy = connect_http(port);
                    healthy.write_all(b"GET /healthy HTTP/1.1\r\nHost: localhost\r\n\r\n").unwrap();
                    let mut bytes = Vec::new();
                    bad.read_to_end(&mut bytes).unwrap();
                    assert!(bytes.is_empty(), "{bytes:?}");
                    let (headers, body) = read_http(&mut healthy);
                    assert!(headers.starts_with("HTTP/1.1 200 "), "{headers}");
                    assert_eq!(body, b"ok");
                }
            });
            let mut command = Command::new(&executable);
            command.env("PXR_PHP", "/pxr-no-php-cli");
            if stress {
                command.env("PXR_GC_STRESS", "1");
            }
            let out = execute(&mut command);
            assert!(
                out.status.success(),
                "{}",
                String::from_utf8_lossy(&out.stderr)
            );
            assert_eq!(out.stdout, b"done\n");
            assert!(out.stderr.is_empty());
            client.join().unwrap();
        }
        std::fs::remove_dir_all(executable.parent().unwrap()).unwrap();
    }
}

#[test]
fn http_listener_failures_still_propagate_to_the_caller() {
    if !have_cc() {
        return;
    }
    let occupied = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let port = occupied.local_addr().unwrap().port();
    for (i, (address, timeout)) in [("127.0.0.1", 1000), ("localhost", 1000), ("127.0.0.1", 0)]
        .into_iter()
        .enumerate()
    {
        let source = format!(
            r#"<?php
async function handle(): string {{ return "ok"; }}
async function main(): void {{
    await httpServeAsync("{address}", {port}, "handle", 1, 4096, {timeout});
    echo "unreachable";
}}"#
        );
        for opt in [0, 3] {
            let executable = build(&format!("http-listener-error-{i}"), &source, opt);
            for stress in [false, true] {
                let mut command = Command::new(&executable);
                command.env("PXR_PHP", "/pxr-no-php-cli");
                if stress {
                    command.env("PXR_GC_STRESS", "1");
                }
                let out = execute(&mut command);
                assert_eq!(out.status.code(), Some(255), "{out:?}");
                assert!(out.stdout.is_empty());
                assert!(String::from_utf8_lossy(&out.stderr).starts_with("pxr: async task failed:"));
            }
            std::fs::remove_dir_all(executable.parent().unwrap()).unwrap();
        }
    }
}

#[test]
fn stop_server_drains_accepted_http_and_tcp_work_and_closes_the_listener() {
    use std::io::{Read, Write};
    if !have_cc() {
        return;
    }
    for http in [false, true] {
        for finite in [false, true] {
            let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            listener.set_nonblocking(true).unwrap();
            let gate_port = listener.local_addr().unwrap().port();
            let port = server_port("127.0.0.1");
            let gate = std::thread::spawn(move || {
                for _ in 0..4 {
                    let mut ready = accept_tcp(&listener);
                    let mut request = Vec::new();
                    ready.read_to_end(&mut request).unwrap();
                    assert_eq!(request, b"ready");
                    let mut client = connect_http(port);
                    if http {
                        client
                            .write_all(b"GET / HTTP/1.1\r\nHost: localhost\r\n\r\n")
                            .unwrap();
                    } else {
                        client.write_all(b"request").unwrap();
                        client.shutdown(std::net::Shutdown::Write).unwrap();
                    }
                    let mut hold = accept_tcp(&listener);
                    request.clear();
                    hold.read_to_end(&mut request).unwrap();
                    assert_eq!(request, b"hold");
                    ready.write_all(b"go").unwrap();
                    drop(ready);
                    let mut stopped = accept_tcp(&listener);
                    request.clear();
                    stopped.read_to_end(&mut request).unwrap();
                    assert_eq!(request, b"stopped");
                    assert!(
                        std::net::TcpStream::connect(("127.0.0.1", port)).is_err(),
                        "listener still accepts after stop"
                    );
                    stopped.write_all(b"ok").unwrap();
                    drop(stopped);
                    hold.write_all(b"done").unwrap();
                    drop(hold);
                    if http {
                        let (headers, body) = read_http(&mut client);
                        assert!(headers.starts_with("HTTP/1.1 200 "), "{headers}");
                        assert_eq!(body, b"done");
                    } else {
                        request.clear();
                        client.read_to_end(&mut request).unwrap();
                        assert_eq!(request, b"done");
                    }
                }
            });
            let serve = if http {
                "httpServeAsync"
            } else {
                "tcpServeAsync"
            };
            let parameters = if http { "" } else { "string $request" };
            let count = if finite { 1 } else { 0 };
            let stop = if finite {
                "halt($server, 10000)"
            } else {
                "halt($server)"
            };
            let source = format!(
                r#"<?php
namespace App {{
use function stopServer as halt;
async function reply({parameters}): string {{
    return await tcpExchangeAsync("127.0.0.1", {gate_port}, "hold", 4, 10000);
}}
async function run(): void {{
    $server = spawn {serve}("127.0.0.1", {port}, "App\\reply", {count}, 64, 10000, 1);
    await tcpExchangeAsync("127.0.0.1", {gate_port}, "ready", 2, 10000);
    {stop};
    await tcpExchangeAsync("127.0.0.1", {gate_port}, "stopped", 2, 10000);
    echo "stopping\n";
}}
}}
namespace {{ async function main(): void {{ await \App\run(); }} }}
"#
            );
            run_case(
                &format!("stop-server-drain-{http}-{finite}"),
                &source,
                "stopping\n",
            );
            gate.join().unwrap();
        }
    }
}

#[test]
fn stop_server_grace_expiry_joins_handlers_before_reusing_their_resources() {
    use std::io::{Read, Write};
    if !have_cc() {
        return;
    }
    for http in [false, true] {
        for grace in [0, 25] {
            let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            listener.set_nonblocking(true).unwrap();
            let gate_port = listener.local_addr().unwrap().port();
            let port = server_port("127.0.0.1");
            let resource = server_port("127.0.0.1");
            let gate = std::thread::spawn(move || {
                for _ in 0..4 {
                    let mut ready = accept_tcp(&listener);
                    let mut request = Vec::new();
                    ready.read_to_end(&mut request).unwrap();
                    assert_eq!(request, b"ready");
                    let mut client = connect_http(port);
                    if http {
                        client
                            .write_all(b"GET / HTTP/1.1\r\nHost: localhost\r\n\r\n")
                            .unwrap();
                    } else {
                        client.write_all(b"request").unwrap();
                        client.shutdown(std::net::Shutdown::Write).unwrap();
                    }
                    let mut hold = accept_tcp(&listener);
                    request.clear();
                    hold.read_to_end(&mut request).unwrap();
                    assert_eq!(request, b"hold");
                    ready.write_all(b"go").unwrap();
                    drop(ready);
                    request.clear();
                    client.read_to_end(&mut request).unwrap();
                    assert!(
                        request.is_empty(),
                        "cancelled handler returned output: {request:?}"
                    );
                    assert!(std::net::TcpStream::connect(("127.0.0.1", port)).is_err());
                    drop(hold);
                }
            });
            let serve = if http {
                "httpServeAsync"
            } else {
                "tcpServeAsync"
            };
            let parameters = if http { "" } else { "string $request" };
            let source = format!(
                r#"<?php
async function echoBack(string $request): string {{ return $request; }}
async function reply({parameters}): string {{
    $resource = spawn tcpServeAsync("127.0.0.1", {resource}, "echoBack", 0, 64);
    return await tcpExchangeAsync("127.0.0.1", {gate_port}, "hold", 4, 10000);
}}
async function scopedServer(): void {{
    $server = spawn {serve}("127.0.0.1", {port}, "reply", 1, 64, 10000, 1);
    await tcpExchangeAsync("127.0.0.1", {gate_port}, "ready", 2, 10000);
    stopServer($server, {grace});
    echo "requested\n";
}}
async function main(): void {{
    await scopedServer();
    $replacement = spawn tcpServeAsync("127.0.0.1", {resource}, "echoBack", 0, 64);
    await sleepAsync(0); cancelTask($replacement);
    echo "joined\n";
}}"#
            );
            run_case(
                &format!("stop-server-expiry-{http}-{grace}"),
                &source,
                "requested\njoined\n",
            );
            gate.join().unwrap();
        }
    }
}

#[test]
fn stop_server_consumes_lazy_handles_without_opening_or_running_them() {
    let port = server_port("127.0.0.1");
    run_case(
        "stop-server-lazy",
        &format!(
            r#"<?php
async function reply(string $request): string {{ echo "unreachable"; return $request; }}
async function handle(): string {{ echo "unreachable"; return "ok"; }}
async function main(): void {{
    $tcp = tcpServeAsync("127.0.0.1", {port}, "reply", 0, 64);
    StOpSeRvEr(move($tcp), 0);
    stopServer(httpServeAsync("127.0.0.1", {port}, "handle", 0, 64));
    await sleepAsync(0); echo "stopped\n";
}}"#
        ),
        "stopped\n",
    );
}

#[test]
fn stop_server_rejects_invalid_source_and_runtime_targets() {
    if !have_cc() {
        return;
    }
    let dir = std::env::temp_dir().join(format!("pxr-stop-server-errors-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    for (i, source) in [
        "function stopServer(): void {}",
        "function f(): void { stopServer(sleepAsync(0)); }",
        "async function main(): void { stopServer(); }",
        "async function main(): void { stopServer(1); }",
        "async function main(): void { stopServer(sleepAsync(0), \"10\"); }",
        "async function f(): string { return \"x\"; } async function main(): void { stopServer(f()); }",
        "async function main(): void { $s = sleepAsync(0); stopServer($s); await $s; }",
        "async function main(): void { $s = sleepAsync(0); stopServer($s); cancelTask($s); }",
        "async function main(): void { stopServer(sleepAsync(0), 1, 2); }",
        "async function main(): void { stopServer<int>(sleepAsync(0)); }",
    ].iter().enumerate() {
        let path = dir.join(format!("{i}.php")); std::fs::write(&path, format!("<?php {source}")).unwrap();
        let out = Command::new(env!("CARGO_BIN_EXE_pxr")).arg("check").arg(path).output().unwrap();
        assert!(!out.status.success(), "case {i} accepted");
        assert!(!String::from_utf8_lossy(&out.stderr).contains("panicked"));
    }
    std::fs::remove_dir_all(dir).unwrap();
    for (i, body) in [
        "stopServer(sleepAsync(0));",
        "$t = spawn sleepAsync(0); await sleepAsync(10); stopServer($t);",
        "stopServer(tcpServeAsync(\"127.0.0.1\", 8080, \"reply\", 0, 64), -1);",
    ]
    .iter()
    .enumerate()
    {
        let source = format!("<?php async function reply(string $r): string {{ return $r; }} async function main(): void {{ {body} await sleepAsync(0); }}");
        for opt in [0, 3] {
            let executable = build(&format!("stop-server-runtime-error-{i}"), &source, opt);
            for stress in [false, true] {
                let mut command = Command::new(&executable);
                command.env("PXR_PHP", "/pxr-no-php-cli");
                if stress {
                    command.env("PXR_GC_STRESS", "1");
                }
                let out = execute(&mut command);
                assert_eq!(out.status.code(), Some(255), "{out:?}");
                assert!(String::from_utf8_lossy(&out.stderr).starts_with("pxr: async task failed:"));
            }
            std::fs::remove_dir_all(executable.parent().unwrap()).unwrap();
        }
    }
}

#[test]
fn recoverable_results() {
    run_case(
        "recoverable_results",
        r#"<?php
class X { public int $value; public function __construct(int $v) { $this->value=$v; } }
async function fail(): int { await sleepAsync(-1); return 1; }
async function make(): owned<X> { await sleepAsync(0); return new X(42); }
async function main(): void {
    $failed = spawn taskResultAsync(fail());
    $made = spawn taskResultAsync(make());
    await sleepAsync(0);
    $a = await $failed; echo resultOk($a), resultStatus($a), ":", resultError($a) > 0, ":";
    drop($a);
    $b = await $made; echo resultOk($b), ":";
    $obj = await resultValueAsync($b); echo $obj->value, "\n";
}
"#,
        "2:1:1:42\n",
    );
}

#[test]
fn streaming_sockets() {
    let socket = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = socket.local_addr().unwrap().port();
    drop(socket);
    let source = r#"<?php
async function server(owned<TcpListener> $listener): void {
    $stream = await tcpAcceptAsync($listener);
    closeSocket($listener);
    $request = await tcpReadAsync($stream, 1024);
    await tcpWriteAsync($stream, "reply:" . $request);
    tcpShutdownWrite($stream); closeSocket($stream);
}
async function connect(): owned<TcpStream> { return await tcpConnectAsync("127.0.0.1", {port}); }
async function main(): void {
    $listener = await tcpListenAsync("127.0.0.1", {port});
    $server = spawn server(move($listener));
    $result = await taskResultAsync(connect());
    if (resultOk($result)) {
        $stream = await resultValueAsync($result);
        await tcpWriteAsync($stream, "hello");
        echo await tcpReadAsync($stream, 1024), "\n";
        closeSocket($stream);
    } else { echo "error:", resultError($result); }
    await $server;
}
"#
    .replace("{port}", &port.to_string());
    run_case("streaming_sockets", &source, "reply:hello\n");
}

#[test]
fn recoverable_io_failures_preserve_siblings_and_owned_payloads() {
    run_case(
        "io-errors",
        r#"<?php
async function fail(): int { await sleepAsync(-1); return 0; }
async function good(): int { await sleepAsync(0); return 42; }
async function main(): void {
    $bad = spawn taskResultAsync(fail());
    $good = spawn good();
    $a = await $bad;
    echo resultStatus($a), ":", resultError($a) > 0, ":", await $good, ":";
    drop($a);
    $invalid = await taskResultAsync(tcpConnectAsync("not-an-address", 80));
    echo resultOk($invalid), resultStatus($invalid), ":", resultError($invalid) > 0, "\n";
    drop($invalid);
}
"#,
        "2:1:42:2:1\n",
    );
}

#[test]
fn isolated_workers_and_duplex_channels_make_concurrent_progress() {
    run_case(
        "parallel-channels",
        include_str!("../../../examples/async/parallel.php"),
        "42:7:12:13:10\n",
    );
}

#[test]
fn workers_transfer_owned_objects_arrays_and_mixed_without_aliasing() {
    run_case(
        "worker-values",
        include_str!("../../../examples/async/parallel_values.php"),
        "42:2:2:42:42\n",
    );
}

#[test]
fn channels_apply_backpressure_and_report_peer_close_timeout_and_busy() {
    run_case(
        "channel-pressure",
        r#"<?php
async function main(): void {
    $a = await channelAsync(1); $b = await channelPeerAsync($a);
    await channelSendAsync($a, "first");
    $second = spawn channelSendAsync($a, "second");
    await sleepAsync(0);
    echo await channelReceiveAsync($b), ":";
    await $second; echo await channelReceiveAsync($b), ":";
    $waiting = spawn taskResultAsync(channelReceiveAsync($b));
    await sleepAsync(0);
    $busy = await taskResultAsync(channelReceiveAsync($b));
    echo resultStatus($busy), ":"; drop($busy);
    $timeout = await taskResultAsync(channelReceiveAsync($a, 0));
    echo resultStatus($timeout), ":"; drop($timeout);
    closeChannel($a);
    $closed = await $waiting;
    echo resultStatus($closed), ":", resultError($closed) > 0, "\n"; drop($closed);
    closeChannel($b);
}
"#,
        "first:second:2:2:2:1\n",
    );
}

#[test]
fn worker_failure_cancellation_lazy_drop_and_channel_transfer_cleanup() {
    run_case(
        "worker-lifetime",
        r#"<?php
async function failed(int $n): int { await sleepAsync(-1); return $n; }
async function slow(owned<Channel> $c): void { await channelReceiveAsync($c); }
async function roundTrip(owned<Channel> $c): owned<Channel> { await sleepAsync(0); return move($c); }
async function main(): void {
    $lazy = parallelAsync("failed", 1); drop($lazy);
    $failed = await taskResultAsync(parallelAsync("failed", 0));
    echo resultStatus($failed), ":"; drop($failed);
    $i = 0;
    while ($i < 20) {
        $a = await channelAsync(1); $b = await channelPeerAsync($a);
        $work = spawn parallelAsync("slow", move($b));
        await sleepAsync(1); cancelTask($work);
        $closed = await taskResultAsync(channelReceiveAsync($a, 1000));
        echo resultOk($closed); drop($closed); closeChannel($a); $i++;
    }
    $a = await channelAsync(); $b = await channelPeerAsync($a);
    $b = await parallelAsync("roundTrip", move($b));
    await channelSendAsync($b, 42); echo await channelReceiveAsync($a), "\n";
    drop($a); drop($b);
}
"#,
        "2:42\n",
    );
}

#[test]
fn invalid_thread_resource_and_result_contracts_are_diagnosed() {
    let cases = [
        "class TaskResult {}",
        "namespace Bad { class TaskResult<T> {} }",
        "class Channel {}",
        "class X extends TcpStream {}",
        "async function main(): void { $r = await taskResultAsync(sleepAsync(0)); $s = $r; }",
        "async function main(): void { $c = await channelAsync(); $d = $c; }",
        "async function main(): void { $c = await channelAsync(); closeChannel($c); await channelReceiveAsync($c); }",
        "async function main(): void { $c = await channelAsync(); await channelSendAsync($c, new X()); } class X {}",
        "async function f(int $n): int {return $n;} async function main(): void { $name = \"f\"; await parallelAsync($name, 1); }",
        "async function f(int $n): int {return $n;} function main(): void { parallelAsync(\"f\", 1); }",
        "async function f(): int {return 0;} async function main(): void { await parallelAsync(\"f\", 1); }",
        "function f(int $n): int {return $n;} async function main(): void { await parallelAsync(\"f\", 1); }",
        "class X {} async function f(X $x): int {return 1;} async function main(): void { await parallelAsync(\"f\", new X()); }",
        "class X {} async function f(int $x): X {return new X();} async function main(): void { await parallelAsync(\"f\", 1); }",
        "async function f(&int $x): int {return $x;} async function main(): void {$x: owned<int> = 1; await parallelAsync(\"f\", &$x);}",
        "async function f(owned<TcpStream> $s): int {return 0;} async function main(): void {$s = await tcpConnectAsync(\"127.0.0.1\", 80); await parallelAsync(\"f\", move($s));}",
        "class Y {} class X {public Y $child;} async function f(owned<X> $x): int {return 0;} async function main(): void {$x: owned<X> = new X(); await parallelAsync(\"f\", move($x));}",
        "class X {} async function make(): owned<X> {return new X();} async function main(): void {$r: owned<TaskResult<X>> = await taskResultAsync(make());}",
        "async function main(): void { closeSocket(); }",
        "async function main(): void { await channelAsync(\"wrong\"); }",
    ];
    let dir = std::env::temp_dir().join(format!("pxr-m7-thread-errors-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    for (i, case) in cases.iter().enumerate() {
        let input = dir.join(format!("{i}.php"));
        std::fs::write(&input, format!("<?php\n{case}")).unwrap();
        let output = Command::new(env!("CARGO_BIN_EXE_pxr"))
            .arg("check")
            .arg(&input)
            .output()
            .unwrap();
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            !output.status.success(),
            "case {i} unexpectedly accepted: {case}"
        );
        assert!(
            stderr.contains("error") && !stderr.contains("panicked"),
            "{i}: {stderr}"
        );
    }
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn workers_preserve_scalar_bits_namespaces_and_nested_task_trees() {
    run_case(
        "worker-scalars",
        r#"<?php
namespace Jobs {
async function number(float $n): float { return $n + 0.5; }
async function negate(bool $b): bool { return !$b; }
async function identity(mixed $m): mixed { return $m; }
async function nested(int $n): int { return await parallelAsync("Jobs\\numberInt", $n); }
async function numberInt(int $n): int { return $n + 1; }
async function numberInt__parallel(int $n): int { return $n; }
}
namespace {
async function main(): void {
    echo await parallelAsync("Jobs\\number", 41.5), ":", await parallelAsync("Jobs\\negate", false), ":";
    echo is_null(await parallelAsync("Jobs\\identity", null)), ":", await parallelAsync("Jobs\\nested", 41), "\n";
}
}
"#,
        "42:1:1:42\n",
    );
}

#[test]
fn recoverable_outcomes_preserve_void_and_rethrow_errors_structurally() {
    run_case(
        "result-rethrow",
        r#"<?php
async function main(): void {
    $ok = await taskResultAsync(sleepAsync(0));
    echo resultOk($ok), ":", resultError($ok), ":";
    await resultValueAsync($ok);
    $bad = await taskResultAsync(sleepAsync(-1));
    $again = await taskResultAsync(resultValueAsync($bad));
    echo resultStatus($again), ":", resultError($again) > 0, "\n";
    drop($again);
}
"#,
        "1:0:2:1\n",
    );
}

#[test]
fn channel_clones_preserve_queued_values_and_dynamic_messages_are_validated() {
    run_case(
        "channel-clones",
        r#"<?php
class X {}
async function main(): void {
    $a = await channelAsync(1); $b = await channelPeerAsync($a);
    $clone = await channelCloneAsync($b); drop($b);
    await channelSendAsync($clone, "last"); closeChannel($clone);
    echo await channelReceiveAsync($a), ":";
    $eof = await taskResultAsync(channelReceiveAsync($a));
    echo resultStatus($eof), ":"; drop($eof); drop($a);
    $a = await channelAsync(); $b = await channelPeerAsync($a);
    $hidden: mixed = new X();
    $invalid = await taskResultAsync(channelSendAsync($a, $hidden));
    echo resultStatus($invalid), ":"; drop($invalid);
    await channelSendAsync($a, ["binary" => "a\0b", "nested" => [42]]);
    $value = await channelReceiveAsync($b);
    if (is_array($value)) {
        echo strlen(strval($value["binary"])), ":";
        $nested = $value["nested"]; if (is_array($nested)) {echo $nested[0];}
    }
    echo "\n"; drop($a); drop($b);
}
"#,
        "last:2:2:3:42\n",
    );
}

#[test]
fn generic_helpers_preserve_channel_and_result_ownership() {
    run_case(
        "generic-resources",
        r#"<?php
async function send<T>(owned<Channel> $c, T $value): owned<Channel> {
    await channelSendAsync($c, $value); return move($c);
}
async function unwrap<T>(owned<TaskResult<T>> $result): T { return await resultValueAsync($result); }
async function answer(): int {return 42;}
async function main(): void {
    $a = await channelAsync(); $b = await channelPeerAsync($a);
    $a = await send(move($a), [42]);
    $value = await channelReceiveAsync($b);
    if (is_array($value)) {echo $value[0], ":";}
    echo await unwrap(await taskResultAsync(answer())), "\n";
    drop($a); drop($b);
}
"#,
        "42:42\n",
    );
}

#[test]
fn string_append_snapshots_survive_returns_borrows_and_task_suspension() {
    run_case(
        "string_append_tasks",
        r#"<?php
function buildAppend(string $text): string {
    for ($i = 0; $i < 1000; $i++) {$text .= "ab";}
    return $text;
}
function inspectAppend(&string $text): int {return strlen($text);}
async function holdAppend(string $text): string {await sleepAsync(0); return $text;}
async function suffixAppend(): string {await sleepAsync(0); return "x";}
async function main(): void {
    $text = buildAppend("start"); $copy = $text;
    $task = spawn holdAppend($text);
    for ($i = 0; $i < 500; $i++) {$text .= "x";}
    await sleepAsync(0);
    $held = await $task;
    $owned: owned<string> = "owned";
    $during = "";
    for ($j = 0; $j < 16; $j++) {$during .= await suffixAppend();}
    echo strlen($text), ":", strlen($copy), ":", strlen($held), ":", inspectAppend(&$owned), ":", strlen($during), "\n";
}
"#,
        "2505:2005:2005:5:16\n",
    );
}

#[test]
fn inline_objects_and_leaf_methods_keep_returns_and_allocating_callbacks_alive() {
    run_case(
        "object_leaf_gc",
        r#"<?php
class InlineNode {
    public string $text;
    public int $value;
    public mixed $next = null;
    public function __construct(string $text, int $value) {$this->text = $text; $this->value = $value;}
    public function textValue(): string {return $this->text;}
    public function bump(): int {$this->value += 1; return $this->value;}
}
function makeInlineText(): string {
    $node = new InlineNode(str_repeat("x", 4096), 7);
    return $node->textValue();
}
function allocatingScalar(InlineNode $node): int {
    $garbage = str_repeat("g", 8192);
    return $node->bump();
}
class TransitiveInlineNode {
    public string $text;
    public function __construct() {$this->text = makeInlineText();}
    public function textValue(): string {return $this->text;}
}
$kept = makeInlineText();
$node = new InlineNode("node", 10); $alias = $node; $node->next = $node;
$parent = new TransitiveInlineNode();
for ($i = 0; $i < 32; $i++) {allocatingScalar($node);}
echo strlen($kept), ":", $alias->value, ":", strlen($parent->textValue()), ":", $node->textValue(), "\n";
"#,
        "4096:42:4096:node\n",
    );
}

#[test]
fn keyed_foreach_keeps_keys_and_nested_values_alive_across_await() {
    run_case(
        "keyed_foreach_await",
        r#"<?php
async function main(): void {
    $values = ["binary\0key" => [7], 9 => [8], "01" => [9]];
    $snapshot = $values;
    foreach ($values as $key => $value) {
        await sleepAsync(0);
        echo is_int($key), ":", $key, ":", $value[0], ";";
        $value[0] = 42; $values[$key] = $value;
    }
    $first = $snapshot["binary\0key"];
    echo $first[0], "\n";
}
"#,
        ":binary\0key:7;1:9:8;:01:9;7\n",
    );
}

#[test]
fn nested_array_property_targets_and_cow_survive_rhs_suspension() {
    run_case(
        "nested_write_await",
        r#"<?php
class AwaitBag {public array<array<int>> $values = [[7, 8]]; public static array<array<int>> $shared = [[3]];}
async function nestedNumber(): int {await sleepAsync(0); return 9;}
function nestedIndex(int $key): int {echo "K"; return $key;}
async function main(): void {
    $bag = new AwaitBag(); $copy = $bag->values; $alias = $bag;
    $bag->values[nestedIndex(0)][nestedIndex(0)] = await nestedNumber();
    AwaitBag::$shared[0][0] += await nestedNumber();
    echo ":", $alias->values[0][0], ":", $copy[0][0], ":", AwaitBag::$shared[0][0], "\n";
    $empty = []; $empty[]["items"][] = await nestedNumber();
    echo $empty[0]["items"][0], "\n";
}
"#,
        "KK:9:7:12\n9\n",
    );
}

#[test]
fn assignment_results_and_interpolation_survive_suspension() {
    run_case(
        "assignment_results",
        r#"<?php
async function result(int $n): int { await sleepAsync(0); return $n; }
async function main(): void {
    $a = [[1]];
    $i = 0;
    echo ($a[$i++][0] += await result(4)), ":", $i, ":", "{$a[0][0]}", "\n";
    echo ($a[0][2] ??= await result(8)), ":", ($a[0][2] ??= await result(99)), "\n";
    $b = ($a[0] = [await result(7)]);
    $b[0]++;
    echo "{$a[0][0]}:{$b[0]}\n";
    $mixed: mixed = [["text"]];
    await sleepAsync(0);
    echo "{$mixed[0][0]}\n";
}
"#,
        "5:1:5\n8:8\n7:8\ntext\n",
    );
}

#[test]
fn invokable_receivers_and_arguments_survive_await_and_gc() {
    run_case(
        "invokable_await",
        r#"<?php
interface Callback { public function __invoke(string $value): string; }
class PrefixCallback implements Callback {
    public string $prefix;
    public function __construct(string $prefix) { $this->prefix = $prefix; }
    public function __invoke(string $value): string { return $this->prefix . $value; }
}
function receiver(Callback $callback): Callback { echo "receiver:"; return $callback; }
async function argument(): string { await sleepAsync(0); return "done"; }
async function main(): void {
    $callback = new PrefixCallback("kept:");
    echo receiver($callback)(await argument()), "\n";
    echo receiver(new PrefixCallback("temporary:"))(await argument()), "\n";
    $callbacks = [$callback];
    foreach ($callbacks as $fn) { await sleepAsync(0); echo $fn("again"), "\n"; }
}
"#,
        "receiver:kept:done\nreceiver:temporary:done\nkept:again\n",
    );
}

#[test]
fn closure_environments_escape_factories_and_survive_suspension() {
    run_case(
        "closure_lifetime",
        r#"<?php
function makeText(string $seed) { return fn(int $n): string => $seed . $n; }
async function number(): int { await sleepAsync(0); return 7; }
async function main(): void {
    $format = makeText("retained:");
    await sleepAsync(0);
    echo $format(await number()), "\n";
    $values = [3];
    $snapshot = function() use ($values): int { $values[0]++; return $values[0]; };
    $values[0] = 99;
    $make = fn(int $x) => fn(int $y): int => $x + $y;
    $inner = $make(5);
    for ($i=0; $i<50; $i++) { $temporary = makeText("temporary:"); await sleepAsync(0); }
    echo $snapshot(), ":", $snapshot(), ":", $values[0], ":", $inner(8), "\n";
}
"#,
        "retained:7\n4:4:99:13\n",
    );
}

#[test]
fn bound_closures_retain_temporary_receivers_across_suspension() {
    run_case(
        "bound_closure_lifetime",
        r#"<?php
class BoundLifetime {
    private string $prefix;
    public function __construct(string $prefix) { $this->prefix = $prefix; }
    public function callback(string $suffix) {
        return function(int $n) use ($suffix): string { return $this->prefix . $suffix . $n; };
    }
    public function nested() { return fn(int $x) => fn(int $y): string => $this->prefix . ($x + $y); }
    public function makeStatic() { return static fn(int $n): int => $n + 1; }
    public async function asyncCallback() { await sleepAsync(0); return fn(int $n): string => $this->prefix . $n; }
}
async function value(): int { await sleepAsync(0); return 7; }
async function main(): void {
    $callback = (new BoundLifetime("kept:"))->callback("!");
    $make = (new BoundLifetime("nested:"))->nested();
    $inner = $make(5);
    $static = (new BoundLifetime("unused:"))->makeStatic();
    $fromAsync = await (new BoundLifetime("async:"))->asyncCallback();
    for ($i=0; $i<50; $i++) { $temporary = new BoundLifetime("temporary"); await sleepAsync(0); }
    echo $callback(await value()), "\n", $inner(await value()), "\n", $static(await value()), "\n", $fromAsync(await value()), "\n";
}
"#,
        "kept:!7\nnested:12\n8\nasync:7\n",
    );
}

#[test]
fn workers_keep_bound_closure_heaps_and_lexical_statics_isolated() {
    run_case(
        "worker_bound_closures",
        r#"<?php
class WorkerClosure {
    private int $value;
    private static int $calls = 0;
    public function __construct(int $value) { $this->value = $value; }
    public function callback() {
        return fn(int $n): int => $this->value + $n + (self::$calls += 1);
    }
}
async function closureWorker(int $n): int {
    $callback = (new WorkerClosure($n))->callback();
    await sleepAsync(0);
    return $callback(10);
}
async function main(): void {
    $a = spawn parallelAsync("closureWorker", 1);
    $b = spawn parallelAsync("closureWorker", 2);
    echo await $a, ":", await $b, "\n";
}
"#,
        "12:13\n",
    );
}

#[test]
fn closure_lexical_types_survive_escaping_frames_and_awaited_arguments() {
    run_case(
        "closure_lexical_types",
        r#"<?php
class LexicalParent { public string $text; public function __construct(string $text) { $this->text=$text; } }
class LexicalChild extends LexicalParent {
    public async function make() {
        await sleepAsync(0);
        return fn(self $value): parent => $value;
    }
    public function retained() { return fn(): self => $this; }
    public static function unbound() { return static fn(parent $value): parent => $value; }
}
async function argument(): LexicalChild { await sleepAsync(0); return new LexicalChild("argument"); }
async function main(): void {
    $callback = await (new LexicalChild("factory"))->make();
    $retained = (new LexicalChild("retained"))->retained();
    $unbound = LexicalChild::unbound();
    for ($i=0; $i<30; $i++) { $garbage = new LexicalChild("temporary"); await sleepAsync(0); }
    echo $callback(await argument())->text, ":", $retained()->text, ":", $unbound(new LexicalParent("parent"))->text, "\n";
}
"#,
        "argument:retained:parent\n",
    );
}
