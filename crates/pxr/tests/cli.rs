//! End-to-end CLI tests.
//!
//! These drive the real `pxr` binary: they compile `.php` sources all the way to
//! native executables and run them. They are skipped automatically when no C
//! compiler is available, since PXR uses clang as its LLVM backend driver.

use std::path::PathBuf;
use std::process::Command;

fn pxr() -> &'static str {
    env!("CARGO_BIN_EXE_pxr")
}

fn have_cc() -> bool {
    let cc = std::env::var("CC").unwrap_or_else(|_| "clang".to_string());
    Command::new(cc)
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// Write `source` to a unique temp file and return its path.
fn temp_php(name: &str, source: &str) -> PathBuf {
    let mut dir = std::env::temp_dir();
    dir.push(format!(
        "pxr-it-{}-{}",
        std::process::id(),
        name.replace('/', "_")
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join(format!("{name}.php"));
    std::fs::write(&path, source).unwrap();
    path
}

struct Run {
    code: i32,
    stdout: String,
    stderr: String,
}

fn run_ok(args: &[&str]) -> Run {
    let out = Command::new(pxr()).args(args).output().unwrap();
    Run {
        code: out.status.code().unwrap_or(-1),
        stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
    }
}

#[test]
fn run_hello() {
    if !have_cc() {
        eprintln!("skipping: no C compiler");
        return;
    }
    let f = temp_php("hello", "<?php echo \"Hello, PXR!\n\";");
    let r = run_ok(&["run", f.to_str().unwrap()]);
    assert_eq!(r.code, 0, "stderr: {}", r.stderr);
    assert_eq!(r.stdout, "Hello, PXR!\n");
}

#[test]
fn build_produces_standalone_executable() {
    if !have_cc() {
        eprintln!("skipping: no C compiler");
        return;
    }
    let f = temp_php("standalone", "<?php echo 40 + 2;");
    let exe = f.with_extension("bin");
    let r = run_ok(&["build", f.to_str().unwrap(), "-o", exe.to_str().unwrap()]);
    assert_eq!(r.code, 0, "stderr: {}", r.stderr);
    let out = Command::new(&exe).output().unwrap();
    assert_eq!(out.status.code(), Some(0));
    assert_eq!(String::from_utf8_lossy(&out.stdout), "42");
}

#[test]
fn recursion_and_types() {
    if !have_cc() {
        eprintln!("skipping: no C compiler");
        return;
    }
    let src = r#"<?php
function fib(int $n): int {
    if ($n < 2) { return $n; }
    return fib($n - 1) + fib($n - 2);
}
echo fib(10);
"#;
    let f = temp_php("fib", src);
    let r = run_ok(&["run", f.to_str().unwrap()]);
    assert_eq!(r.code, 0, "stderr: {}", r.stderr);
    assert_eq!(r.stdout, "55");
}

#[test]
fn short_circuit_is_correct() {
    if !have_cc() {
        eprintln!("skipping: no C compiler");
        return;
    }
    let src = r#"<?php
function mark(string $s): bool {
    echo $s;
    return true;
}
$f = false;
$t = true;
$a = $f && mark("X");   // short-circuits: X not printed
$b = $t || mark("Y");   // short-circuits: Y not printed
$c = $t && mark("Z");   // evaluates: Z printed
echo "\n";
"#;
    let f = temp_php("shortcircuit", src);
    let r = run_ok(&["run", f.to_str().unwrap()]);
    assert_eq!(r.code, 0, "stderr: {}", r.stderr);
    assert_eq!(r.stdout, "Z\n");
}

#[test]
fn check_reports_type_errors_with_exit_code() {
    let f = temp_php("bad", "<?php echo $undefined;");
    let r = run_ok(&["check", f.to_str().unwrap()]);
    assert_eq!(r.code, 1);
    assert!(r.stderr.contains("E0220"), "stderr: {}", r.stderr);
    assert!(r.stderr.contains("undefined variable"));
    assert!(r.stderr.contains("^"));
}

#[test]
fn check_reports_syntax_errors() {
    let f = temp_php("syntax", "<?php echo ;");
    let r = run_ok(&["check", f.to_str().unwrap()]);
    assert_eq!(r.code, 1);
    assert!(r.stderr.contains("error"));
}

#[test]
fn emit_ir_contains_functions() {
    let f = temp_php(
        "ir",
        "<?php function add(int $a, int $b): int { return $a + $b; } echo add(1,2);",
    );
    let r = run_ok(&["emit-ir", f.to_str().unwrap()]);
    assert_eq!(r.code, 0, "stderr: {}", r.stderr);
    assert!(r.stdout.contains("func pxr$fn$add"));
    assert!(r.stdout.contains("func main"));
}

#[test]
fn emit_llvm_is_llvm_assembly() {
    let f = temp_php("ll", "<?php echo 1;");
    let r = run_ok(&["emit-llvm", f.to_str().unwrap()]);
    assert_eq!(r.code, 0, "stderr: {}", r.stderr);
    assert!(r.stdout.contains("define i32 @main()"));
    assert!(r.stdout.contains("declare void @pxr_print_i64"));
}

#[test]
fn type_error_prevents_build() {
    if !have_cc() {
        eprintln!("skipping: no C compiler");
        return;
    }
    let f = temp_php(
        "typeerr",
        "<?php function f(int $a): int { return $a; } echo f(\"x\");",
    );
    let r = run_ok(&["run", f.to_str().unwrap()]);
    assert_eq!(r.code, 1);
    assert!(r.stderr.contains("E0231"), "stderr: {}", r.stderr);
}

#[test]
fn division_is_float_and_modulo_is_int() {
    if !have_cc() {
        eprintln!("skipping: no C compiler");
        return;
    }
    let f = temp_php("divmod", "<?php echo (10 / 4) . \" \" . (10 % 4);");
    let r = run_ok(&["run", f.to_str().unwrap()]);
    assert_eq!(r.code, 0, "stderr: {}", r.stderr);
    assert_eq!(r.stdout, "2.5 2");
}

#[test]
fn builtins_work() {
    if !have_cc() {
        eprintln!("skipping: no C compiler");
        return;
    }
    let src = r#"<?php
echo strlen("hello") . " ";
echo intval("42abc") . " ";
echo strval(3.5) . " ";
echo abs(-9) . " ";
echo floatval("1.25");
"#;
    let f = temp_php("builtins", src);
    let r = run_ok(&["run", f.to_str().unwrap()]);
    assert_eq!(r.code, 0, "stderr: {}", r.stderr);
    assert_eq!(r.stdout, "5 42 3.5 9 1.25");
}

#[test]
fn arrays_and_foreach() {
    if !have_cc() {
        eprintln!("skipping: no C compiler");
        return;
    }
    let src = r#"<?php
$a = [3, 1, 4, 1, 5];
$a[] = 9;
echo count($a) . " " . $a[0] . " " . $a[5] . "\n";
$sum = 0;
foreach ($a as $v) {
    $sum += $v;
}
echo $sum . "\n";
$m = ["x" => 10, "y" => 20];
echo $m["x"] + $m["y"] . "\n";
"#;
    let f = temp_php("arrays", src);
    let r = run_ok(&["run", f.to_str().unwrap()]);
    assert_eq!(r.code, 0, "stderr: {}", r.stderr);
    assert_eq!(r.stdout, "6 3 9\n23\n30\n");
}

#[test]
fn array_element_type_refinement() {
    if !have_cc() {
        eprintln!("skipping: no C compiler");
        return;
    }
    // Regression: the assignment that refines an empty `array<null>` must be
    // stored using the *refined* element type, not the placeholder `null`.
    let src = r#"<?php
$a = [];
$a[] = "x";
$a[] = "y";
echo $a[0] . $a[1] . "\n";
$b = [];
$b[3] = 4;
$b[1] = 2;
echo $b[3] . " " . $b[1] . " " . count($b) . "\n";
$c = [1, 2.5, 3];
echo $c[0] . " " . $c[1] . " " . $c[2] . "\n";
"#;
    let f = temp_php("refine", src);
    let r = run_ok(&["run", f.to_str().unwrap()]);
    assert_eq!(r.code, 0, "stderr: {}", r.stderr);
    assert_eq!(r.stdout, "xy\n4 2 2\n1 2.5 3\n");
}

#[test]
fn control_flow_breadth() {
    if !have_cc() {
        eprintln!("skipping: no C compiler");
        return;
    }
    let src = r#"<?php
$s = 0;
for ($i = 0; $i < 10; $i++) {
    if ($i == 3) { continue; }
    if ($i == 7) { break; }
    $s += $i;
}
echo $s . "\n";
$n = 3;
$fact = 1;
while ($n > 1) {
    $fact *= $n;
    $n--;
}
echo $fact . "\n";
$j = 0;
do {
    $j++;
} while ($j < 3);
echo $j . "\n";
echo (5 <=> 2) . (5 <=> 5) . (2 <=> 5) . "\n";
$r = match (2) {
    1 => "a",
    2 => "b",
    default => "c",
};
echo $r . "\n";
switch (1) {
    case 1:
        echo "one";
        break;
    default:
        echo "other";
}
echo "\n";
echo (5 & 3) . " " . (6 | 1) . " " . (5 ^ 1) . " " . (2 << 3) . " " . (16 >> 2) . " " . (~0) . "\n";
"#;
    let f = temp_php("controlflow", src);
    let r = run_ok(&["run", f.to_str().unwrap()]);
    assert_eq!(r.code, 0, "stderr: {}", r.stderr);
    assert_eq!(r.stdout, "18\n6\n3\n10-1\nb\none\n1 7 4 16 4 -1\n");
}

/// The M1 acceptance test: a PHP quicksort compiles and sorts correctly.
#[test]
fn quicksort_matches_php() {
    if !have_cc() {
        eprintln!("skipping: no C compiler");
        return;
    }
    let src = r#"<?php
$a = [5, 3, 8, 4, 2, 7, 1, 10, 9, 6];
$stack = [];
$sp = 0;
$stack[$sp] = 0;
$sp = $sp + 1;
$stack[$sp] = count($a) - 1;
$sp = $sp + 1;
while ($sp > 0) {
    $sp = $sp - 1;
    $hi = $stack[$sp];
    $sp = $sp - 1;
    $lo = $stack[$sp];
    if ($lo >= $hi) { continue; }
    $pivot = $a[$hi];
    $i = $lo - 1;
    for ($j = $lo; $j < $hi; $j++) {
        if ($a[$j] < $pivot) {
            $i = $i + 1;
            $tmp = $a[$i];
            $a[$i] = $a[$j];
            $a[$j] = $tmp;
        }
    }
    $i = $i + 1;
    $tmp = $a[$i];
    $a[$i] = $a[$hi];
    $a[$hi] = $tmp;
    $stack[$sp] = $lo; $sp = $sp + 1;
    $stack[$sp] = $i - 1; $sp = $sp + 1;
    $stack[$sp] = $i + 1; $sp = $sp + 1;
    $stack[$sp] = $hi; $sp = $sp + 1;
}
foreach ($a as $v) {
    echo $v . " ";
}
echo "\n";
"#;
    let f = temp_php("quicksort", src);
    let r = run_ok(&["run", f.to_str().unwrap()]);
    assert_eq!(r.code, 0, "stderr: {}", r.stderr);
    assert_eq!(r.stdout, "1 2 3 4 5 6 7 8 9 10 \n");
}

#[test]
fn array_misuse_is_rejected() {
    let f = temp_php("badarray", "<?php $a = [1, 2]; echo $a;");
    let r = run_ok(&["check", f.to_str().unwrap()]);
    assert_eq!(r.code, 1);
    assert!(r.stderr.contains("E0240"), "stderr: {}", r.stderr);

    let f = temp_php(
        "badforeach",
        "<?php $a = [1, 2]; foreach ($a as $k => $v) { echo $v; }",
    );
    let r = run_ok(&["check", f.to_str().unwrap()]);
    assert_eq!(r.code, 1);
    assert!(r.stderr.contains("E0242"), "stderr: {}", r.stderr);
}

#[test]
fn emit_llvm_declares_array_runtime() {
    let f = temp_php("llarr", "<?php $a = [1]; echo $a[0];");
    let r = run_ok(&["emit-llvm", f.to_str().unwrap()]);
    assert_eq!(r.code, 0, "stderr: {}", r.stderr);
    assert!(r.stdout.contains("declare ptr @pxr_array_new"));
    assert!(r.stdout.contains("declare i64 @pxr_array_get_bits"));
}

/// The shared-subset example must produce byte-identical output under PXR and
/// the PHP interpreter (skipped when PHP is not installed).
#[test]
fn compat_example_matches_php() {
    if !have_cc() {
        eprintln!("skipping: no C compiler");
        return;
    }
    let have_php = Command::new("php")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);
    if !have_php {
        eprintln!("skipping: no php interpreter");
        return;
    }
    let file = concat!(env!("CARGO_MANIFEST_DIR"), "/../../examples/compat.php");
    let r = run_ok(&["run", file]);
    assert_eq!(r.code, 0, "stderr: {}", r.stderr);
    let php = Command::new("php").arg(file).output().unwrap();
    assert_eq!(
        r.stdout,
        String::from_utf8_lossy(&php.stdout),
        "PXR and PHP output differ"
    );
}

/// The milestone-1 acceptance example runs from the source tree.
#[test]
fn quicksort_example_runs() {
    if !have_cc() {
        eprintln!("skipping: no C compiler");
        return;
    }
    let file = concat!(env!("CARGO_MANIFEST_DIR"), "/../../examples/quicksort.php");
    let r = run_ok(&["run", file]);
    assert_eq!(r.code, 0, "stderr: {}", r.stderr);
    assert_eq!(r.stdout, "1 2 3 4 5 6 7 8 9 10 \n");
}
