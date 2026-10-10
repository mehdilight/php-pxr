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
    let r = run_ok(&[
        "build",
        f.to_str().unwrap(),
        "-o",
        exe.to_str().unwrap(),
    ]);
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
    let f = temp_php("ir", "<?php function add(int $a, int $b): int { return $a + $b; } echo add(1,2);");
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
    let f = temp_php("typeerr", "<?php function f(int $a): int { return $a; } echo f(\"x\");");
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
