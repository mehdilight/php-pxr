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

fn have_php() -> bool {
    Command::new("php")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// Compile and run `src` with PXR, run it with the PHP interpreter, and assert
/// the two produce byte-identical stdout. Both are skipped when the tools are
/// unavailable.
fn assert_matches_php(name: &str, src: &str) {
    if !have_cc() {
        eprintln!("skipping: no C compiler");
        return;
    }
    if !have_php() {
        eprintln!("skipping: no php interpreter");
        return;
    }
    let f = temp_php(name, src);
    let r = run_ok(&["run", f.to_str().unwrap()]);
    assert_eq!(r.code, 0, "stderr: {}", r.stderr);
    let php = Command::new("php").arg(&f).output().unwrap();
    assert!(
        php.status.success(),
        "PHP failed for `{name}`: {}{}",
        String::from_utf8_lossy(&php.stdout),
        String::from_utf8_lossy(&php.stderr)
    );
    assert_eq!(
        r.stdout,
        String::from_utf8_lossy(&php.stdout),
        "PXR and PHP output differ for `{name}`"
    );
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
fn php_faithful_array_value_semantics() {
    if !have_cc() {
        eprintln!("skipping: no C compiler");
        return;
    }
    // Every case here is valid PHP *and* valid PXR; PXR must match PHP:
    //   - `$b = $a` copies, and mutating `$b` leaves `$a` alone;
    //   - `foreach` iterates a snapshot, so appending inside the loop is unseen;
    //   - arrays nested in an array literal are copied;
    //   - canonical integer string keys unify with integer keys;
    //   - `match`'s `default` is a fallback regardless of position;
    //   - shift counts >= 64 yield 0 (or -1 for `>>` of a negative value).
    let src = r#"<?php
$a = [1, 2, 3];
$b = $a;
$b[0] = 99;
echo $a[0] . "\n";

$e = [1, 2, 3];
foreach ($e as $v) {
    $e[] = 100;
}
echo count($e) . "\n";

$inner = [1];
$outer = [$inner];
$inner[] = 2;
echo count($outer[0]) . " " . count($inner) . "\n";

$k = [];
$k["1"] = 10;
$k[1] = 20;
echo count($k) . " " . $k["1"] . " " . $k[1] . "\n";

$m = match (true) { default => "lead", true => "spur" };
echo $m . "\n";

echo (1 << 64) . " " . (-1 >> 64) . " " . (1 >> 64) . " " . (2 << 2) . "\n";
"#;
    let f = temp_php("valuesem", src);
    let r = run_ok(&["run", f.to_str().unwrap()]);
    assert_eq!(r.code, 0, "stderr: {}", r.stderr);
    assert_eq!(r.stdout, "1\n6\n1 2\n1 20 20\nspur\n0 -1 0 8\n");
}

#[test]
fn array_argument_is_copied() {
    if !have_cc() {
        eprintln!("skipping: no C compiler");
        return;
    }
    // PHP passes arrays by value; the callee's mutation must not be visible to
    // the caller. `array<int>` is PXR syntax (PHP has no generic array hint).
    let src = r#"<?php
function bump(array<int> $x): array<int> {
    $x[0] = $x[0] + 1;
    return $x;
}
$c = [7, 8];
$d = bump($c);
echo $c[0] . " " . $d[0] . "\n";
"#;
    let f = temp_php("argcopy", src);
    let r = run_ok(&["run", f.to_str().unwrap()]);
    assert_eq!(r.code, 0, "stderr: {}", r.stderr);
    assert_eq!(r.stdout, "7 8\n");
}

#[test]
fn negative_shift_is_a_fatal_error() {
    if !have_cc() {
        eprintln!("skipping: no C compiler");
        return;
    }
    // PHP raises an uncaught ArithmeticError (exit status 255); PXR reproduces
    // the message and exit status.
    let f = temp_php("negshift", "<?php echo 1 << -1;");
    let r = run_ok(&["run", f.to_str().unwrap()]);
    assert_eq!(r.code, 255, "stderr: {}", r.stderr);
    assert!(
        r.stderr.contains("Bit shift by negative number"),
        "stderr: {}",
        r.stderr
    );
}

#[test]
fn unsupported_interpolation_forms_are_rejected() {
    // Simple `$x` / `{$x}` interpolation works, but PHP's array, property and
    // `${...}` forms cannot be expanded yet. Rather than silently emitting the
    // literal (which would diverge from PHP), each is a hard compile error.
    for (name, src) in [
        ("iarr", "<?php $a = [1]; echo \"$a[0]\";"),
        ("iprop", "<?php $o = 1; echo \"$o->p\";"),
        ("icplx", "<?php $a = [1]; echo \"{$a[0]}\";"),
        ("idoll", "<?php $x = 1; echo \"${x}\";"),
    ] {
        let f = temp_php(name, src);
        let r = run_ok(&["check", f.to_str().unwrap()]);
        assert_eq!(r.code, 1, "src {src}: {}", r.stderr);
        assert!(r.stderr.contains("E0008"), "src {src}: {}", r.stderr);
    }
}

#[test]
fn php_faithful_string_comparison() {
    if !have_cc() {
        eprintln!("skipping: no compiler");
        return;
    }
    // PHP compares two numeric strings by value in loose comparisons (`==`,
    // ordering, `<=>`, and `switch`), but `===` is always byte equality.
    let src = r#"<?php
echo (("10" < "9") ? "lt" : "ge") . "\n";
echo (("1" == "01") ? "loose" : "ne") . " " . (("1" === "01") ? "strict" : "strictne") . "\n";
echo (("abc" < "abd") ? "lt" : "ge") . "\n";
echo ("10" <=> "9") . " " . (7 <=> 7) . "\n";
$s = "1";
switch ($s) {
    case "01":
        echo "loose-case\n";
        break;
    default:
        echo "default\n";
}
"#;
    let f = temp_php("strcmp", src);
    let r = run_ok(&["run", f.to_str().unwrap()]);
    assert_eq!(r.code, 0, "stderr: {}", r.stderr);
    assert_eq!(r.stdout, "ge\nloose strictne\nlt\n1 0\nloose-case\n");
}

#[test]
fn php_faithful_float_formatting() {
    if !have_cc() {
        eprintln!("skipping: no C compiler");
        return;
    }
    // Values must stringify exactly like PHP (zend_gcvt, precision 14):
    // "1.0E+15" not "1E+15", "1.0E-5" not "1E-05", integral floats without a
    // fraction, "-0" for negative zero, and large literals must compile.
    let src = r#"<?php
echo 0.1 . "\n";
echo (0.1 + 0.2) . "\n";
echo (1.0 / 3.0) . "\n";
echo 1e14 . "\n";
echo 1e16 . "\n";
echo 1e-5 . "\n";
echo 1e300 . "\n";
echo 2.0 . "\n";
echo 100.0 . "\n";
echo 123456789.123456 . "\n";
echo -0.0 . "\n";
"#;
    let f = temp_php("floats", src);
    let r = run_ok(&["run", f.to_str().unwrap()]);
    assert_eq!(r.code, 0, "stderr: {}", r.stderr);
    assert_eq!(
        r.stdout,
        "0.1\n0.3\n0.33333333333333\n1.0E+14\n1.0E+16\n1.0E-5\n1.0E+300\n2\n100\n123456789.12346\n-0\n"
    );
}

#[test]
fn division_by_zero_is_fatal() {
    if !have_cc() {
        eprintln!("skipping: no C compiler");
        return;
    }
    // PHP raises an uncaught DivisionByZeroError (exit 255) for `/` and `%`.
    let f = temp_php("divzero", "<?php echo 1 / 0;");
    let r = run_ok(&["run", f.to_str().unwrap()]);
    assert_eq!(r.code, 255, "stderr: {}", r.stderr);
    assert!(
        r.stderr.contains("Division by zero"),
        "stderr: {}",
        r.stderr
    );

    let f2 = temp_php("modzero", "<?php echo 5 % 0;");
    let r2 = run_ok(&["run", f2.to_str().unwrap()]);
    assert_eq!(r2.code, 255, "stderr: {}", r2.stderr);
    assert!(
        r2.stderr.contains("Division by zero"),
        "stderr: {}",
        r2.stderr
    );
}

#[test]
fn string_with_embedded_nul() {
    if !have_cc() {
        eprintln!("skipping: no C compiler");
        return;
    }
    // PHP strings are byte strings: `strlen` counts bytes including embedded
    // NULs, `===` compares bytes, ordering is byte-exact, and concatenation
    // preserves them. (The old `char *` representation truncated at NUL.)
    let src = r#"<?php
echo strlen("a\0b") . "\n";
echo ("a\0b" === "a\0b") ? "eq\n" : "ne\n";
echo ("a\0b" === "a\0c") ? "eq\n" : "ne\n";
echo ("a\0b" === "a") ? "eq\n" : "ne\n";
echo strlen("a\0b" . "c") . "\n";
echo ("a\0b" < "a\0c") ? "lt\n" : "ge\n";
echo ("0\0" ? "truthy\n" : "falsy\n");
echo ("\0" ? "nul-truthy\n" : "nul-falsy\n");
echo strlen("\0\0\0") . "\n";
"#;
    let f = temp_php("nulstr", src);
    let r = run_ok(&["run", f.to_str().unwrap()]);
    assert_eq!(r.code, 0, "stderr: {}", r.stderr);
    assert_eq!(r.stdout, "3\neq\nne\nne\n4\nlt\ntruthy\nnul-truthy\n3\n");
}

#[test]
fn string_builtins_match_php() {
    if !have_cc() {
        eprintln!("skipping: no C compiler");
        return;
    }
    let src = r#"<?php
echo strtoupper("Hello, World 123!") . "\n";
echo strtoupper("héllo") . "\n";
echo strtolower("HeLLo 123!") . "\n";
echo "[" . trim("  hello  \n\t") . "]\n";
echo "[" . trim("xxhelloxy", "xy") . "]\n";
echo "[" . trim(" \0 ab \0 ") . "]\n";
echo "[" . ltrim("  hi  ") . "]\n";
echo "[" . rtrim("  hi  ") . "]\n";
echo "[" . rtrim("hello///", "/") . "]\n";
echo "[" . trim(" x ", "") . "]\n";
echo "[" . trim("\x0b x \x0b") . "]\n";
echo substr("abcdef", 1) . "\n";
echo substr("abcdef", -4) . "\n";
echo substr("abcdef", 2, 3) . "\n";
echo substr("abcdef", -3, -1) . "\n";
echo substr("abcdef", 4) . "\n";
echo substr("abcdef", 0, 99) . "\n";
echo substr("abcdef", 4, -1) . "\n";
echo substr("abcdef", -99) . "\n";
echo substr("abcdef", 2, 0) . "\n";
echo "[" . substr("a", 2) . "]\n";
echo "[" . substr("abcdef", 6) . "]\n";
echo str_repeat("ab", 3) . "\n";
echo str_repeat("ab", 0) . "\n";
echo str_replace("l", "L", "hello world") . "\n";
echo str_replace("", "x", "abc") . "\n";
echo str_replace("a", "", "abc") . "\n";
echo str_replace("ab", "aab", "abab") . "\n";
echo str_replace("aa", "x", "aaaa") . "\n";
echo substr("a\0b", 1, 1) === "\0" ? "nul-sub ok\n" : "nul-sub bad\n";
echo strtoupper("a\0z") === "A\0Z" ? "nul-case ok\n" : "nul-case bad\n";
"#;
    let f = temp_php("m2strings", src);
    let r = run_ok(&["run", f.to_str().unwrap()]);
    assert_eq!(r.code, 0, "stderr: {}", r.stderr);
    let expected = "\
HELLO, WORLD 123!
HéLLO
hello 123!
[hello]
[hello]
[ab]
[hi  ]
[  hi]
[hello]
[ x ]
[x]
bcdef
cdef
cde
de
ef
abcdef
e
abcdef

[]
[]
ababab

heLLo worLd
abc
bc
aabaab
xx
nul-sub ok
nul-case ok
";
    assert_eq!(r.stdout, expected);
}

#[test]
fn explode_implode_match_php() {
    if !have_cc() {
        eprintln!("skipping: no C compiler");
        return;
    }
    let src = r#"<?php
$e = explode(",", "a,b,c");
echo "[" . implode("|", $e) . "] count=" . count($e) . "\n";
$e = explode(",", "a,b,c", 2);
echo "[" . implode("|", $e) . "] count=" . count($e) . "\n";
$e = explode(",", "a,b,c", -1);
echo "[" . implode("|", $e) . "] count=" . count($e) . "\n";
$e = explode(",", "a,b,c", 0);
echo "[" . implode("|", $e) . "] count=" . count($e) . "\n";
$e = explode(",", "abc");
echo "[" . implode("|", $e) . "] count=" . count($e) . "\n";
$e = explode("x", "");
echo "[" . implode("|", $e) . "] count=" . count($e) . "\n";
$e = explode(",", ",a,");
echo "[" . implode("|", $e) . "] count=" . count($e) . "\n";
$e = explode(",", ",a,", 2);
echo "[" . implode("|", $e) . "] count=" . count($e) . "\n";
$e = explode(",", "a,,,");
echo "[" . implode("|", $e) . "] count=" . count($e) . "\n";
$e = explode(",", "a,,,", -2);
echo "[" . implode("|", $e) . "] count=" . count($e) . "\n";
$e = explode(",", "a,b", -5);
echo "[" . implode("|", $e) . "] count=" . count($e) . "\n";
$e = explode("::", "a::b::c");
echo "[" . implode("|", $e) . "] count=" . count($e) . "\n";
echo implode(" ", ["a", "b"]) . "\n";
echo implode(["a", "b", "c"]) . "\n";
echo implode("-", [1, 2, 3]) . "\n";
echo implode("-", [1.5, 2]) . "\n";
echo implode("|", [true, false]) . "\n";
$w = explode(" ", "the quick brown fox jumps over the lazy dog");
echo count($w) . " words, longest: " . strtoupper($w[4]) . "\n";
"#;
    let f = temp_php("m2explode", src);
    let r = run_ok(&["run", f.to_str().unwrap()]);
    assert_eq!(r.code, 0, "stderr: {}", r.stderr);
    let expected = "\
[a|b|c] count=3
[a|b,c] count=2
[a|b] count=2
[a,b,c] count=1
[abc] count=1
[] count=1
[|a|] count=3
[|a,] count=2
[a|||] count=4
[a|] count=2
[] count=0
[a|b|c] count=3
a b
abc
1-2-3
1.5-2
1|
9 words, longest: JUMPS
";
    assert_eq!(r.stdout, expected);
}

#[test]
fn str_repeat_negative_is_fatal() {
    if !have_cc() {
        eprintln!("skipping: no C compiler");
        return;
    }
    let f = temp_php("repneg", "<?php echo str_repeat(\"a\", -1);");
    let r = run_ok(&["run", f.to_str().unwrap()]);
    assert_eq!(r.code, 255, "stderr: {}", r.stderr);
    assert!(
        r.stderr
            .contains("str_repeat(): Argument #2 ($times) must be greater than or equal to 0"),
        "stderr: {}",
        r.stderr
    );
}

#[test]
fn explode_empty_separator_is_fatal() {
    if !have_cc() {
        eprintln!("skipping: no C compiler");
        return;
    }
    let f = temp_php(
        "exempty",
        "<?php echo implode(\"|\", explode(\"\", \"ab\"));",
    );
    let r = run_ok(&["run", f.to_str().unwrap()]);
    assert_eq!(r.code, 255, "stderr: {}", r.stderr);
    assert!(
        r.stderr
            .contains("explode(): Argument #1 ($separator) must not be empty"),
        "stderr: {}",
        r.stderr
    );
}

#[test]
fn string_predicates_and_case_match_php() {
    if !have_cc() {
        eprintln!("skipping: no C compiler");
        return;
    }
    let src = r#"<?php
echo (str_contains("hello world", "wor") ? "T" : "F");
echo (str_contains("hello", "xyz") ? "T" : "F");
echo (str_contains("hello", "") ? "T" : "F");
echo (str_contains("", "a") ? "T" : "F");
echo (str_contains("a\0b", "\0") ? "T" : "F") . "\n";
echo (str_starts_with("hello", "he") ? "T" : "F");
echo (str_starts_with("hello", "") ? "T" : "F");
echo (str_starts_with("he", "hello") ? "T" : "F") . "\n";
echo (str_ends_with("hello", "lo") ? "T" : "F");
echo (str_ends_with("a\0b", "\0b") ? "T" : "F") . "\n";
echo strrev("hello") . "|" . strrev("") . "|" . strrev("ab") . "\n";
echo (strrev("a\0b") === "b\0a" ? "revnul ok" : "revnul bad") . "\n";
echo ucfirst("hello world") . "|" . ucfirst("") . "|" . lcfirst("Hello World") . "\n";
echo ucwords("hello world foo") . "|" . ucwords("  a  b  ") . "|" . ucwords("foo-bar baz") . "\n";
echo (ucfirst("h\xC3\xA9llo") === "H\xC3\xA9llo" ? "ucfnb ok" : "ucfnb bad") . "\n";
"#;
    let f = temp_php("m2pred", src);
    let r = run_ok(&["run", f.to_str().unwrap()]);
    assert_eq!(r.code, 0, "stderr: {}", r.stderr);
    let expected = "\
TFTFT
TTF
TT
olleh||ba
revnul ok
Hello world||hello World
Hello World Foo|  A  B  |Foo-bar Baz
ucfnb ok
";
    assert_eq!(r.stdout, expected);
}

#[test]
fn str_split_matches_php() {
    if !have_cc() {
        eprintln!("skipping: no C compiler");
        return;
    }
    let src = r#"<?php
$p = str_split("abcdef");
echo implode("|", $p) . " n=" . count($p) . "\n";
$p = str_split("abcdef", 2);
echo implode("|", $p) . "\n";
$p = str_split("abcdef", 4);
echo implode("|", $p) . "\n";
$p = str_split("abcdef", 10);
echo implode("|", $p) . "\n";
$p = str_split("");
echo "empty n=" . count($p) . "\n";
"#;
    let f = temp_php("m2split", src);
    let r = run_ok(&["run", f.to_str().unwrap()]);
    assert_eq!(r.code, 0, "stderr: {}", r.stderr);
    assert_eq!(
        r.stdout,
        "a|b|c|d|e|f n=6\nab|cd|ef\nabcd|ef\nabcdef\nempty n=0\n"
    );
}

#[test]
fn array_values_reverse_match_php() {
    if !have_cc() {
        eprintln!("skipping: no C compiler");
        return;
    }
    let src = r#"<?php
$v1 = array_values([10, 20, 30]);
echo $v1[0] . "," . $v1[1] . "," . $v1[2] . " n=" . count($v1) . "\n";
$v2 = array_values(["a" => 1, "b" => 2]);
echo $v2[0] . "," . $v2[1] . "\n";
$v3 = array_values([]);
echo "empty n=" . count($v3) . "\n";
$r1 = array_reverse([10, 20, 30]);
echo $r1[0] . "," . $r1[1] . "," . $r1[2] . "\n";
$r2 = array_reverse(["a" => 1, "b" => 2, "c" => 3]);
echo $r2["a"] . "," . $r2["c"] . " n=" . count($r2) . "\n";
$r3 = array_reverse([5 => "x", 9 => "y"]);
echo $r3[0] . "," . $r3[1] . "\n";
$r4 = array_reverse([1, 2, 3], true);
echo $r4[0] . "," . $r4[1] . "," . $r4[2] . "\n";
"#;
    let f = temp_php("m2arr", src);
    let r = run_ok(&["run", f.to_str().unwrap()]);
    assert_eq!(r.code, 0, "stderr: {}", r.stderr);
    assert_eq!(
        r.stdout,
        "10,20,30 n=3\n1,2\nempty n=0\n30,20,10\n1,3 n=3\ny,x\n1,2,3\n"
    );
}

#[test]
fn str_split_nonpositive_length_is_fatal() {
    if !have_cc() {
        eprintln!("skipping: no C compiler");
        return;
    }
    let f = temp_php(
        "sszero",
        "<?php $p = str_split(\"abc\", 0); echo count($p);",
    );
    let r = run_ok(&["run", f.to_str().unwrap()]);
    assert_eq!(r.code, 255, "stderr: {}", r.stderr);
    assert!(
        r.stderr
            .contains("str_split(): Argument #2 ($length) must be greater than 0"),
        "stderr: {}",
        r.stderr
    );
}

/// Regression: string literals are byte strings, so `\xHH` for `HH >= 0x80`
/// must produce that raw byte, not a UTF-8 replacement character. The lexer
/// used to run the literal through `String::from_utf8_lossy`.
#[test]
fn string_literal_high_bytes_are_raw() {
    if !have_cc() {
        eprintln!("skipping: no C compiler");
        return;
    }
    let src = r#"<?php
echo (strlen("\xA9\xC3") === 2 ? "lenok" : "lenbad") . "\n";
echo (strrev("\xA9\xC3") === "\xC3\xA9" ? "revok" : "revbad") . "\n";
echo (strlen("\x00\x01\xFE\xFF") === 4 ? "len4ok" : "len4bad") . "\n";
"#;
    let f = temp_php("bytes", src);
    let r = run_ok(&["run", f.to_str().unwrap()]);
    assert_eq!(r.code, 0, "stderr: {}", r.stderr);
    assert_eq!(r.stdout, "lenok\nrevok\nlen4ok\n");
}

/// Double-quoted interpolation for `$name` / `{$name}`, including the cases
/// PHP treats as literal `$` (`"$"`, `"$1"`, `"foo$"`, `"$$x"`), escaped
/// dollars, and the string conversion of int/float/bool values.
#[test]
fn string_interpolation_matches_php() {
    if !have_cc() {
        eprintln!("skipping: no C compiler");
        return;
    }
    let src = r#"<?php
$x = 5;
$y = "Y";
$f = 3.5;
$b = true;
echo "simple:[$x] mixed:[a$x b] two:[$x$y]\n";
echo "braced:[{$x}] adj:[{$x}{$y}]\n";
echo "float:[$f] bool:[$b]\n";
echo "dollar:[$] d1:[$1] ds:[$ ] fooend:[foo$]\n";
echo "esc:[\$x] thenlit:[$x.] brace:[a{b] plus:[$x+1]\n";
echo "dollarvar:[$$x]\n";
echo "empty:[]\n";
"#;
    let f = temp_php("interp_ok", src);
    let r = run_ok(&["run", f.to_str().unwrap()]);
    assert_eq!(r.code, 0, "stderr: {}", r.stderr);
    let expected = "simple:[5] mixed:[a5 b] two:[5Y]\n\
                    braced:[5] adj:[5Y]\n\
                    float:[3.5] bool:[1]\n\
                    dollar:[$] d1:[$1] ds:[$ ] fooend:[foo$]\n\
                    esc:[$x] thenlit:[5.] brace:[a{b] plus:[5+1]\n\
                    dollarvar:[$5]\n\
                    empty:[]\n";
    assert_eq!(r.stdout, expected);
}

/// Interpolating a value with no string conversion (an array) is a loud type
/// error, never PHP's `"Array"` warning-and-continue.
#[test]
fn array_interpolation_is_a_loud_type_error() {
    let f = temp_php("interp_arr", "<?php $a = [1, 2]; echo \"arr: $a\";");
    let r = run_ok(&["check", f.to_str().unwrap()]);
    assert_eq!(r.code, 1, "stderr: {}", r.stderr);
    assert!(r.stderr.contains("E0223"), "stderr: {}", r.stderr);
}

/// Escaping & encoding builtins: `addslashes`/`stripslashes`, `bin2hex`,
/// `base64_encode`, and the four URL codecs. Compared byte-for-byte against the
/// PHP interpreter over a range of inputs, including high bytes and NULs.
#[test]
fn escaping_and_encoding_builtins_match_php() {
    let src = r#"<?php
$A = "!*'();:@&=+\$,/?#[]~-_. abcXYZ019\x00\x01\x1f\x7f\x80\xa9\xff";
echo bin2hex(addslashes($A)) . "\n";
echo bin2hex(stripslashes("a\\'b\\\"c\\\\d\\0e")) . "\n";
echo bin2hex(stripslashes("abc\\")) . "\n";
echo bin2hex(bin2hex($A)) . "\n";
echo base64_encode("") . "|" . base64_encode("f") . "|" .
    base64_encode("fo") . "|" . base64_encode("foo") . "|" .
    base64_encode("\xff\xfe\xfd") . "\n";
echo urlencode($A) . "\n";
echo rawurlencode($A) . "\n";
echo urldecode("a%20b+c%2Bd") . "|" . rawurldecode("a%20b+c%2Bd") . "\n";
echo bin2hex(urldecode(urlencode($A))) . "\n";
echo bin2hex(rawurldecode(rawurlencode($A))) . "\n";
echo urldecode("a%2G%z%b") . "|" . urldecode("ab%4") . "\n";
"#;
    assert_matches_php("enc", src);
}

#[test]
fn encoding_builtin_type_errors_are_loud() {
    let f = temp_php("enc_err", "<?php echo base64_encode(123);");
    let r = run_ok(&["check", f.to_str().unwrap()]);
    assert_eq!(r.code, 1, "stderr: {}", r.stderr);
    assert!(r.stderr.contains("E0234"), "stderr: {}", r.stderr);
}

#[test]
fn sprintf_and_printf_match_php() {
    let src = r#"<?php
echo sprintf("%s %d %.2f %x %X %o %b %c %%", "world", 42, 3.14159, 255, 255, 42, 42, 65) . "\n";
echo sprintf("[%10s] [%-10s] [%05d] [%-5d] [%+d] [%+d]", "hi", "hi", 7, 7, 7, -7) . "\n";
echo sprintf("[%'*10s] [%08b] [%.3s] [%10.4s] [%-10.4s]", "bar", 42, "abcdef", "abcdef", "abcdef") . "\n";
echo sprintf("%2\$s %1\$s %s", "first", "second") . "\n";
echo sprintf("%s %s %s %s", 123, 4.5, true, false) . "\n";
echo sprintf("nul=%s\n", "a\x00b");
echo sprintf("empty: [%s]\n", "");
$n = printf("printed %d bytes\n", 42);
echo "ret=$n\n";
"#;
    assert_matches_php("sprintf_printf", src);
}

#[test]
fn sprintf_missing_args_is_fatal() {
    if !have_cc() {
        eprintln!("skipping: no C compiler");
        return;
    }
    let f = temp_php("sprintf_few", r#"<?php echo sprintf("%s %s", "one");"#);
    let r = run_ok(&["run", f.to_str().unwrap()]);
    assert_eq!(r.code, 255, "stderr: {}", r.stderr);
    assert!(
        r.stderr
            .contains("ArgumentCountError: 3 arguments are required, 2 given"),
        "stderr: {}",
        r.stderr
    );
}

#[test]
fn sprintf_unknown_specifier_is_fatal() {
    if !have_cc() {
        eprintln!("skipping: no C compiler");
        return;
    }
    let f = temp_php("sprintf_unk", r#"<?php echo sprintf("%q", "foo");"#);
    let r = run_ok(&["run", f.to_str().unwrap()]);
    assert_eq!(r.code, 255, "stderr: {}", r.stderr);
    assert!(
        r.stderr
            .contains("ValueError: Unknown format specifier \"q\""),
        "stderr: {}",
        r.stderr
    );
}

#[test]
fn sprintf_type_errors_are_loud() {
    let f = temp_php("spf_err0", "<?php echo sprintf();");
    let r = run_ok(&["check", f.to_str().unwrap()]);
    assert_eq!(r.code, 1, "stderr: {}", r.stderr);
    assert!(r.stderr.contains("E0233"), "stderr: {}", r.stderr);

    let f2 = temp_php("spf_err1", "<?php echo sprintf(123);");
    let r2 = run_ok(&["check", f2.to_str().unwrap()]);
    assert_eq!(r2.code, 1, "stderr: {}", r2.stderr);
    assert!(r2.stderr.contains("E0234"), "stderr: {}", r2.stderr);

    let f3 = temp_php("spf_err2", "<?php echo sprintf(\"%s\", [1, 2]);");
    let r3 = run_ok(&["check", f3.to_str().unwrap()]);
    assert_eq!(r3.code, 1, "stderr: {}", r3.stderr);
    assert!(r3.stderr.contains("E0234"), "stderr: {}", r3.stderr);
}

#[test]
fn array_merge_matches_php() {
    if !have_cc() {
        eprintln!("skipping: no C compiler");
        return;
    }
    let src = r#"<?php
$m = array_merge([1,2,3],[4,5]);
echo implode(",", $m) . "\n";
$m2 = array_merge(["a" => 1, "b" => 2], ["b" => 9, "c" => 3]);
echo $m2["a"] . $m2["b"] . $m2["c"] . count($m2) . "\n";
$m3 = array_merge([5 => "p"]);
echo $m3[0] . count($m3) . "\n";
$m4 = array_merge([5 => "p", "a" => "q"]);
echo $m4[0] . $m4["a"] . "\n";
$m5 = array_merge([1,2], []);
echo implode(",", $m5) . "\n";
$a = [1, 2];
$b = array_merge($a, [3]);
$b[0] = 99;
echo $a[0] . "," . $b[0] . "\n";
"#;
    let f = temp_php("m23merge", src);
    let r = run_ok(&["run", f.to_str().unwrap()]);
    assert_eq!(r.code, 0, "stderr: {}", r.stderr);
    assert_eq!(r.stdout, "1,2,3,4,5\n1933\np1\npq\n1,2\n1,99\n");
}

#[test]
fn array_slice_pad_fill_match_php() {
    if !have_cc() {
        eprintln!("skipping: no C compiler");
        return;
    }
    let src = r#"<?php
$s1 = array_slice([1,2,3,4,5], 1);
echo implode(",", $s1) . "\n";
$s2 = array_slice([1,2,3,4,5], 1, 2);
echo implode(",", $s2) . "\n";
$s3 = array_slice([1,2,3,4,5], -2);
echo implode(",", $s3) . "\n";
$s4 = array_slice([1,2,3,4,5], 1, -1);
echo implode(",", $s4) . "\n";
$s5 = array_slice([1,2,3], 5);
echo count($s5) . "\n";
$s6 = array_slice(["a" => 1, "b" => 2, "c" => 3], 1);
echo $s6["b"] . $s6["c"] . count($s6) . "\n";
$s7 = array_slice([1,2,3,4,5], 1, 2, true);
echo $s7[1] . $s7[2] . "\n";
$p1 = array_pad([1,2,3], 5, 0);
echo implode(",", $p1) . "\n";
$p2 = array_pad([1,2,3], -5, 0);
echo implode(",", $p2) . "\n";
$p3 = array_pad([1,2,3], 2, 0);
echo implode(",", $p3) . "\n";
$p4 = array_pad([1.5], 3, 0);
echo implode(",", $p4) . "\n";
$f1 = array_fill(0, 3, "x");
echo implode(",", $f1) . "\n";
$f2 = array_fill(5, 2, 7);
echo $f2[5] . $f2[6] . "\n";
$f3 = array_fill(-2, 3, "z");
echo $f3[-2] . $f3[-1] . $f3[0] . "\n";
$f4 = array_fill(3, 0, "q");
echo count($f4) . "\n";
$k = [1, 2, 3];
echo (array_key_exists(0, $k) ? "1" : "0");
echo (array_key_exists(3, $k) ? "1" : "0");
echo (array_key_exists("1", $k) ? "1" : "0") . "\n";
$sk = ["a" => 1];
echo (array_key_exists("a", $sk) ? "1" : "0");
echo (array_key_exists("b", $sk) ? "1" : "0") . "\n";
"#;
    let f = temp_php("m23slice", src);
    let r = run_ok(&["run", f.to_str().unwrap()]);
    assert_eq!(r.code, 0, "stderr: {}", r.stderr);
    assert_eq!(
        r.stdout,
        "2,3,4,5\n2,3\n4,5\n2,3,4\n0\n232\n23\n\
         1,2,3,0,0\n0,0,1,2,3\n1,2,3\n1.5,0,0\nx,x,x\n77\nzzz\n0\n101\n10\n"
    );
}

#[test]
fn str_pad_and_substr_count_match_php() {
    if !have_cc() {
        eprintln!("skipping: no C compiler");
        return;
    }
    let src = r#"<?php
echo "[" . str_pad("abc", 5) . "]\n";
echo "[" . str_pad("abc", 5, "*") . "]\n";
echo "[" . str_pad("abc", 5, "*", 0) . "]\n";
echo "[" . str_pad("abc", 6, "*", 2) . "]\n";
echo "[" . str_pad("a", 4, "xy", 2) . "]\n";
echo "[" . str_pad("abc", 2) . "]\n";
echo "[" . str_pad("", 4, "*") . "]\n";
echo substr_count("hello world", "o") . "\n";
echo substr_count("aaaa", "aa") . "\n";
echo substr_count("hello", "l", 3) . "\n";
echo substr_count("hello", "l", 0, 3) . "\n";
echo substr_count("hello", "l", 0, -2) . "\n";
echo substr_count("hello", "l", -2) . "\n";
echo substr_count("abcabc", "abc") . "\n";
echo substr_count("ababab", "aba") . "\n";
echo substr_count("hello", "xyz") . "\n";
echo substr_count("hello", "l", 5) . "\n";
"#;
    let f = temp_php("m23pad", src);
    let r = run_ok(&["run", f.to_str().unwrap()]);
    assert_eq!(r.code, 0, "stderr: {}", r.stderr);
    assert_eq!(
        r.stdout,
        "[abc  ]\n[abc**]\n[**abc]\n[*abc**]\n[xaxy]\n[abc]\n[****]\n\
         2\n2\n1\n1\n1\n1\n2\n1\n0\n0\n"
    );
}

#[test]
fn array_fill_negative_count_is_fatal() {
    if !have_cc() {
        eprintln!("skipping: no C compiler");
        return;
    }
    let f = temp_php(
        "afillneg",
        "<?php $x = array_fill(0, -1, 1); echo count($x);",
    );
    let r = run_ok(&["run", f.to_str().unwrap()]);
    assert_eq!(r.code, 255, "stderr: {}", r.stderr);
    assert!(
        r.stderr
            .contains("array_fill(): Argument #2 ($count) must be greater than or equal to 0"),
        "stderr: {}",
        r.stderr
    );
}

#[test]
fn str_pad_empty_pad_is_fatal() {
    if !have_cc() {
        eprintln!("skipping: no C compiler");
        return;
    }
    let f = temp_php("strpadempty", r#"<?php echo str_pad("abc", 5, "");"#);
    let r = run_ok(&["run", f.to_str().unwrap()]);
    assert_eq!(r.code, 255, "stderr: {}", r.stderr);
    assert!(
        r.stderr
            .contains("str_pad(): Argument #3 ($pad_string) must not be empty"),
        "stderr: {}",
        r.stderr
    );
}

#[test]
fn substr_count_argument_errors_are_fatal() {
    if !have_cc() {
        eprintln!("skipping: no C compiler");
        return;
    }
    // An empty needle is checked before the offset, matching PHP.
    let f = temp_php("scempty", r#"<?php echo substr_count("abc", "", 100);"#);
    let r = run_ok(&["run", f.to_str().unwrap()]);
    assert_eq!(r.code, 255, "stderr: {}", r.stderr);
    assert!(
        r.stderr
            .contains("substr_count(): Argument #2 ($needle) must not be empty"),
        "stderr: {}",
        r.stderr
    );

    let f = temp_php("scoff", r#"<?php echo substr_count("hello", "l", 100);"#);
    let r = run_ok(&["run", f.to_str().unwrap()]);
    assert_eq!(r.code, 255, "stderr: {}", r.stderr);
    assert!(
        r.stderr.contains(
            "substr_count(): Argument #3 ($offset) must be contained in argument #1 ($haystack)"
        ),
        "stderr: {}",
        r.stderr
    );

    let f = temp_php("sclen", r#"<?php echo substr_count("hello", "l", 0, 100);"#);
    let r = run_ok(&["run", f.to_str().unwrap()]);
    assert_eq!(r.code, 255, "stderr: {}", r.stderr);
    assert!(
        r.stderr.contains(
            "substr_count(): Argument #4 ($length) must be contained in argument #1 ($haystack)"
        ),
        "stderr: {}",
        r.stderr
    );
}

#[test]
fn array_merge_mixed_element_types_rejected() {
    // PXR arrays are homogeneous: merging an `array<int>` with an `array<float>`
    // (or `array<string>`) is a loud compile error, not a silent mismatch.
    let f = temp_php("m23mix", "<?php $x = array_merge([1,2], [1.5]);");
    let r = run_ok(&["check", f.to_str().unwrap()]);
    assert_ne!(r.code, 0);
    assert!(r.stderr.contains("E0253"), "stderr: {}", r.stderr);
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

/// The milestone-3.1 acceptance test: classes, properties, methods, instantiation,
/// $this, and encapsulation match PHP 8.4 byte-for-byte.
#[test]
fn classes_and_objects_match_php() {
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
    let file = concat!(env!("CARGO_MANIFEST_DIR"), "/../../examples/objects.php");
    let r = run_ok(&["run", file]);
    assert_eq!(r.code, 0, "stderr: {}", r.stderr);
    let php = Command::new("php").arg(file).output().unwrap();
    assert_eq!(
        r.stdout,
        String::from_utf8_lossy(&php.stdout),
        "PXR and PHP output differ on examples/objects.php"
    );
}

#[test]
fn class_visibility_private_rejected() {
    let src = r#"<?php
        class Secret {
            private int $val = 42;
        }
        $s = new Secret();
        echo $s->val;
    "#;
    let file = temp_php("private_prop_rejected", src);
    let r = run_ok(&["check", file.to_str().unwrap()]);
    assert_ne!(r.code, 0);
    assert!(r.stderr.contains("E0265") || r.stdout.contains("E0265"));
}

#[test]
fn class_assign_this_rejected() {
    let src = r#"<?php
        class Foo {
            public function test(): void {
                $this = 10;
            }
        }
    "#;
    let file = temp_php("assign_this_rejected", src);
    let r = run_ok(&["check", file.to_str().unwrap()]);
    assert_ne!(r.code, 0);
    assert!(r.stderr.contains("E0267") || r.stdout.contains("E0267"));
}

#[test]
fn class_type_errors_are_loud() {
    // 1. Unknown class
    let src1 = r#"<?php $x = new NonExistentClass();"#;
    let f1 = temp_php("unknown_class", src1);
    let r1 = run_ok(&["check", f1.to_str().unwrap()]);
    assert_ne!(r1.code, 0);
    assert!(r1.stderr.contains("E0205") || r1.stdout.contains("E0205"));

    // 2. Unknown property
    let src2 = r#"<?php class A { public int $x; } $a = new A(); echo $a->nope;"#;
    let f2 = temp_php("unknown_prop", src2);
    let r2 = run_ok(&["check", f2.to_str().unwrap()]);
    assert_ne!(r2.code, 0);
    assert!(r2.stderr.contains("E0261") || r2.stdout.contains("E0261"));

    // 3. Unknown method
    let src3 = r#"<?php class B { public int $x; } $b = new B(); $b->nope();"#;
    let f3 = temp_php("unknown_method", src3);
    let r3 = run_ok(&["check", f3.to_str().unwrap()]);
    assert_ne!(r3.code, 0);
    assert!(r3.stderr.contains("E0262") || r3.stdout.contains("E0262"));

    // 4. Constructor arity mismatch
    let src4 = r#"<?php class C { public function __construct(int $a) {} } $c = new C();"#;
    let f4 = temp_php("ctor_arity", src4);
    let r4 = run_ok(&["check", f4.to_str().unwrap()]);
    assert_ne!(r4.code, 0);
    assert!(r4.stderr.contains("E0207") || r4.stdout.contains("E0207"));
}

#[test]
fn inheritance_and_polymorphism_match_php() {
    if !have_cc() {
        eprintln!("skipping: no C compiler");
        return;
    }
    let src = r#"<?php
class Base {
    public string $name;
    protected int $val;

    public function __construct(string $name, int $val) {
        $this->name = $name;
        $this->val = $val;
    }

    public function greet(): string {
        return "Base: " . $this->name . " (" . $this->val . ")";
    }
}

class Sub extends Base {
    public function __construct(string $name, int $val) {
        parent::__construct($name, $val * 2);
    }

    public function greet(): string {
        return "Sub: " . parent::greet() . "!";
    }
}

function runGreet(Base $b): void {
    echo $b->greet() . "\n";
}

$b = new Base("Alpha", 10);
$s = new Sub("Beta", 10);
runGreet($b);
runGreet($s);
"#;
    let f = temp_php("inheritance_poly", src);
    let r = run_ok(&["run", f.to_str().unwrap()]);
    assert_eq!(r.code, 0, "stderr: {}", r.stderr);
    let expected = "\
Base: Alpha (10)
Sub: Base: Beta (20)!
";
    assert_eq!(r.stdout, expected);
}

#[test]
fn instanceof_hierarchy_matches_php() {
    if !have_cc() {
        eprintln!("skipping: no C compiler");
        return;
    }
    let src = r#"<?php
class GrandParent {}
class ParentClass extends GrandParent {}
class ChildClass extends ParentClass {}
class OtherClass {}

$c = new ChildClass();
$p = new ParentClass();
$o = new OtherClass();

echo ($c instanceof ChildClass ? "1" : "0") . "\n";
echo ($c instanceof ParentClass ? "1" : "0") . "\n";
echo ($c instanceof GrandParent ? "1" : "0") . "\n";
echo ($c instanceof OtherClass ? "1" : "0") . "\n";
echo ($p instanceof ChildClass ? "1" : "0") . "\n";
echo ($p instanceof GrandParent ? "1" : "0") . "\n";
echo ($o instanceof GrandParent ? "1" : "0") . "\n";
"#;
    let f = temp_php("instanceof_check", src);
    let r = run_ok(&["run", f.to_str().unwrap()]);
    assert_eq!(r.code, 0, "stderr: {}", r.stderr);
    let expected = "\
1
1
1
0
0
1
0
";
    assert_eq!(r.stdout, expected);
}

#[test]
fn inheritance_diagnostics_rejected() {
    // 1. Extending non-existent class (E0205)
    let src1 = r#"<?php class A extends NonExistent {}"#;
    let f1 = temp_php("extend_nonexistent", src1);
    let r1 = run_ok(&["check", f1.to_str().unwrap()]);
    assert_ne!(r1.code, 0);
    assert!(r1.stderr.contains("E0205") || r1.stdout.contains("E0205"));

    // 2. Self inheritance (E0201 / E0205)
    let src2 = r#"<?php class SelfExt extends SelfExt {}"#;
    let f2 = temp_php("self_extend", src2);
    let r2 = run_ok(&["check", f2.to_str().unwrap()]);
    assert_ne!(r2.code, 0);
    assert!(r2.stderr.contains("E0201") || r2.stderr.contains("E0205"));

    // 3. Narrowing visibility (E0265)
    let src3 = r#"<?php
class ParentVis { public function foo(): void {} }
class ChildVis extends ParentVis { protected function foo(): void {} }
"#;
    let f3 = temp_php("narrow_vis", src3);
    let r3 = run_ok(&["check", f3.to_str().unwrap()]);
    assert_ne!(r3.code, 0);
    assert!(r3.stderr.contains("E0265") || r3.stdout.contains("E0265"));

    // 4. Overriding signature mismatch (E0208)
    let src4 = r#"<?php
class ParentSig { public function foo(int $a): void {} }
class ChildSig extends ParentSig { public function foo(string $a): void {} }
"#;
    let f4 = temp_php("sig_mismatch", src4);
    let r4 = run_ok(&["check", f4.to_str().unwrap()]);
    assert_ne!(r4.code, 0);
    assert!(r4.stderr.contains("E0208") || r4.stdout.contains("E0208"));

    // 5. Instanceof with non-object operand (E0260)
    let src5 = r#"<?php
class TargetCls {}
$x = 42;
if ($x instanceof TargetCls) { echo "bad"; }
"#;
    let f5 = temp_php("instanceof_nonobj", src5);
    let r5 = run_ok(&["check", f5.to_str().unwrap()]);
    assert_ne!(r5.code, 0);
    assert!(r5.stderr.contains("E0260") || r5.stdout.contains("E0260"));
}

#[test]
fn abstract_classes_and_dispatch_match_php() {
    let src = include_str!("../../../examples/abstract.php");
    if !have_cc() {
        return;
    }
    let f = temp_php("abstract_classes", src);
    let r = run_ok(&["run", f.to_str().unwrap()]);
    assert_eq!(r.code, 0, "{}", r.stderr);
    assert_eq!(
        r.stdout,
        "square\narea=16\nshape\narea=1\nshape\narea=3.14\nshape\n"
    );
    assert_matches_php("abstract_classes_php", src);
}

#[test]
fn abstract_declarations_without_concrete_descendants_build() {
    if !have_cc() {
        return;
    }
    let f = temp_php(
        "abstract_unused",
        r#"<?php
abstract class Root {
    abstract public function value(): string;
    public function describe(): string { return $this->value(); }
}
abstract class Child extends Root { abstract public function value(): string; }
echo "ok\n";
"#,
    );
    let r = run_ok(&["run", f.to_str().unwrap()]);
    assert_eq!(r.code, 0, "{}", r.stderr);
    assert_eq!(r.stdout, "ok\n");
}

#[test]
fn abstract_class_diagnostics_are_rejected() {
    let cases = [
        ("instantiate", "abstract class A {} $a = new A();", "E0269"),
        ("missing", "abstract class A { abstract public function f(): int; } class B extends A {}", "E0269"),
        ("not_abstract", "class A { abstract public function f(): int; }", "E0269"),
        ("private", "abstract class A { abstract private function f(): int; }", "E0269"),
        ("parent", "abstract class A { abstract public function f(): int; } class B extends A { public function f(): int { return parent::f(); } }", "E0269"),
        ("concrete_parent", "class A { public function f(): int { return 1; } } abstract class B extends A { abstract public function f(): int; }", "E0269"),
        ("signature", "abstract class A { abstract public function f(int $n): int; } class B extends A { public function f(string $n): int { return 1; } }", "E0208"),
        ("visibility", "abstract class A { abstract public function f(): int; } class B extends A { protected function f(): int { return 1; } }", "E0265"),
        ("body", "abstract class A { abstract public function f(): int { return 1; } }", "E0100"),
        ("property", "abstract class A { abstract public int $x; }", "E0100"),
        ("constructor", "abstract class A { abstract public function __construct(int $n); } class B extends A { public function __construct(string $n) {} }", "E0208"),
    ];
    for (name, src, code) in cases {
        let f = temp_php(&format!("abstract_{name}"), src);
        let r = run_ok(&["check", f.to_str().unwrap()]);
        assert_ne!(r.code, 0, "accepted {name}");
        assert!(r.stderr.contains(code), "{name}: {}", r.stderr);
    }
}

#[test]
fn abstract_constructors_and_modifier_order_match_php() {
    assert_matches_php(
        "abstract_constructors",
        r#"<?php
AbStRaCt class Base {
    public abstract function __construct(int $value);
    protected abstract function value(): int;
    public function show(): void { echo $this->value() . "\n"; }
}
class Concrete extends Base {
    private int $n;
    public function __construct(int $value) { $this->n = $value; }
    protected function value(): int { return $this->n; }
}
class Child extends Concrete {}
$a = new Child(42);
$a->show();
"#,
    );
}

#[test]
fn interfaces_dispatch_and_membership_match_php() {
    if !have_cc() {
        return;
    }
    let src = include_str!("../../../examples/interfaces.php");
    let f = temp_php("interfaces", src);
    let r = run_ok(&["run", f.to_str().unwrap()]);
    assert_eq!(r.code, 0, "{}", r.stderr);
    assert_eq!(
        r.stdout,
        "human:10\nnamed\nranked\nhuman:20\nnamed\nranked\nrobot:30\nnamed\nunranked\nhuman\n"
    );
    assert_matches_php("interfaces_php", src);
}

#[test]
fn interface_inheritance_and_covariant_returns_match_php() {
    assert_matches_php(
        "interface_covariance",
        r#"<?php
class Animal {}
class Dog extends Animal {}
interface Factory { function make(): Animal; }
interface DogFactory { function make(): Dog; }
interface Combined extends Factory, DogFactory { function make(): Dog; }
interface ChildFactory extends Combined {}
interface OrderedFactory extends DogFactory, Factory {}
class Maker implements ChildFactory { public function make(): Dog { return new Dog(); } }
function create(ChildFactory $f): Animal { return $f->make(); }
echo (create(new Maker()) instanceof Dog ? "dog" : "other") . "\n";
interface Constructible { function __construct(int $n); function value(): int; }
class Built implements Constructible {
    private int $n;
    public function __construct(int $n) { $this->n = $n; }
    public function value(): int { return $this->n; }
}
function rebuild(Constructible $v): void { $v->__construct(7); echo $v->value() . "\n"; }
rebuild(new Built(3));
"#,
    );
}

#[test]
fn unused_interfaces_build() {
    if !have_cc() {
        return;
    }
    let f = temp_php(
        "unused_interfaces",
        r#"<?php
interface ParentContract { function value(): string; }
interface ChildContract extends ParentContract {}
function unused(ChildContract $c): string { return $c->value(); }
class ForwardImplementation implements ForwardContract { public function n(): int { return 1; } }
interface ForwardContract { function n(): int; }
interface Marker {}
class Tagged implements Marker {}
echo (new Tagged() instanceof Marker ? "yes" : "no") . "\n";
"#,
    );
    let r = run_ok(&["run", f.to_str().unwrap()]);
    assert_eq!(r.code, 0, "{}", r.stderr);
    assert_eq!(r.stdout, "yes\n");
}

#[test]
fn invalid_interface_programs_are_rejected() {
    let cases = [
        ("missing", "interface I { function f(): int; } class A implements I {}", "E0269"),
        ("unknown", "class A implements Unknown {}", "E0205"),
        ("class_extends", "interface I {} class A extends I {}", "E0270"),
        ("class_implements", "class A {} class B implements A {}", "E0270"),
        ("interface_extends", "class A {} interface I extends A {}", "E0270"),
        ("cycle", "interface I extends J {} interface J extends I {}", "E0201"),
        ("self", "interface I extends I {}", "E0201"),
        ("duplicate", "interface I {} class A implements I, i {}", "E0270"),
        ("collision", "interface I {} class I {}", "E0201"),
        ("private", "interface I { private function f(): int; }", "E0270"),
        ("protected", "interface I { protected function f(): int; }", "E0270"),
        ("visibility", "interface I { function f(): int; } class A implements I { protected function f(): int { return 1; } }", "E0265"),
        ("signature", "interface I { function f(int $n): int; } class A implements I { public function f(string $n): int { return 1; } }", "E0208"),
        ("conflict", "interface I { function f(): int; } interface J { function f(): string; } interface K extends I, J {}", "E0208"),
        ("numeric_return", "interface I { function f(): float; } class A implements I { public function f(): int { return 1; } }", "E0208"),
        ("inherited_covariance_order", "class A {} class B extends A {} interface I { function f(): A; } interface J { function f(): B; } interface K extends I, J {}", "E0208"),
        ("abstract_covariance_order", "class A {} class B extends A {} interface I { function f(): A; } interface J { function f(): B; } abstract class K implements I, J {}", "E0208"),
        ("inherited", "class A { public function f(): string { return \"a\"; } } interface I { function f(): int; } class B extends A implements I {}", "E0208"),
        ("constructor", "interface I { function __construct(int $n); } class A implements I { public function __construct(string $n) {} }", "E0208"),
        ("body", "interface I { function f(): int { return 1; } }", "E0100"),
        ("property", "interface I { public int $x; }", "E0100"),
        ("abstract_modifier", "interface I { abstract function f(): int; }", "E0100"),
        ("instantiate", "interface I {} $a = new I();", "E0269"),
        ("not_implemented", "interface I {} class A {} function f(I $i): void {} f(new A());", "E0231"),
    ];
    for (name, source, code) in cases {
        let f = temp_php(&format!("interface_invalid_{name}"), source);
        let r = run_ok(&["check", f.to_str().unwrap()]);
        assert_ne!(r.code, 0, "accepted {name}");
        assert!(r.stderr.contains(code), "{name}: {}", r.stderr);
    }
}

#[test]
fn static_members_and_inheritance_match_php() {
    if !have_cc() {
        return;
    }
    let src = include_str!("../../../examples/static.php");
    let f = temp_php("static_members", src);
    let r = run_ok(&["run", f.to_str().unwrap()]);
    assert_eq!(r.code, 0, "{}", r.stderr);
    assert_eq!(
        r.stdout,
        "1\n12\ncounter:2\n102\ncounter:2\nPXR runtime:3:1\n1:9\n99\n3\n"
    );
    assert_matches_php("static_members_php", src);
}

#[test]
fn static_methods_have_no_this_and_bind_self_lexically() {
    assert_matches_php(
        "static_binding",
        r#"<?php
class ParentCounter {
    public static int $value = 1;
    public static function read(): int { return self::$value; }
    public function instanceValue(): int { return 7; }
    public function invoke(): int { return self::instanceValue(); }
}
class ChildCounter extends ParentCounter {
    public static int $value = 2;
    public static function read(): int { return parent::read() + self::$value; }
    public function invoke(): int { return parent::invoke(); }
}
echo ChildCounter::read() . "\n";
ParentCounter::$value = 5;
echo ChildCounter::read() . "\n";
echo ChildCounter::$value . "\n";
$p = new ChildCounter();
echo $p->invoke() . "\n";
abstract class Creator { abstract public static function make(): int; }
class ConcreteCreator extends Creator { public static function make(): int { return 8; } }
echo ConcreteCreator::make() . "\n";
class StaticFactory {
    public static function sum(float $a, int $b): float { return $a + $b; }
    public static function make(): ParentCounter { return new ParentCounter(); }
}
echo StaticFactory::sum(2, 3) . "\n";
echo StaticFactory::make()->invoke() . "\n";
"#,
    );
}

#[test]
fn static_properties_initialize_once_and_copy_arrays() {
    assert_matches_php(
        "static_initialization",
        r#"<?php
class ChildSettings extends BaseSettings {}
class BaseSettings {
    public static int $count = 1 + 2;
    public static $words = ["hello", "world"];
    static public function first(): string { return self::$words[0]; }
}
ChildSettings::$count++;
echo BaseSettings::$count . ":" . ChildSettings::$count . "\n";
$a = ["new", "value"];
ChildSettings::$words = $a;
$a[0] = "changed";
echo ChildSettings::first() . ":" . $a[0] . "\n";
$x = new BaseSettings();
$y = new ChildSettings();
echo BaseSettings::$count . "\n";
"#,
    );
}

#[test]
fn static_member_diagnostics_are_rejected() {
    let cases = [
        ("this", "class A { public static function f(): int { return $this->x; } }", "E0268"),
        ("instance_call", "class A { public function f(): int { return 1; } } A::f();", "E0271"),
        ("static_to_instance", "class A { public function f(): int { return 1; } public static function g(): int { return self::f(); } }", "E0271"),
        ("private_property", "class A { private static int $x = 1; } echo A::$x;", "E0265"),
        ("private_method", "class A { private static function f(): int { return 1; } } A::f();", "E0265"),
        ("protected_property", "class A { protected static int $x = 1; } A::$x = 2;", "E0265"),
        ("instance_property", "class A { public int $x = 1; } echo A::$x;", "E0271"),
        ("object_property", "class A { public static int $x = 1; } $a = new A(); echo $a->x;", "E0271"),
        ("object_method", "class A { public static function f(): int { return 1; } } $a = new A(); $a->f();", "E0271"),
        ("property_override", "class A { public static int $x = 1; } class B extends A { public int $x = 1; }", "E0271"),
        ("method_override", "class A { public static function f(): int { return 1; } } class B extends A { public function f(): int { return 2; } }", "E0271"),
        ("interface", "interface I { static function f(): int; } class A implements I { public function f(): int { return 1; } }", "E0271"),
        ("constructor", "class A { public static function __construct() {} }", "E0271"),
        ("no_default", "class A { public static int $x; }", "E0271"),
        ("dynamic_default", "class A { public static int $x = intval(1); }", "E0271"),
        ("wrong_default", "class A { public static int $x = \"bad\"; }", "E0212"),
        ("wrong_assignment", "class A { public static int $x = 1; } A::$x = \"bad\";", "E0212"),
        ("arity", "class A { public static function f(int $x): int { return $x; } } A::f();", "E0207"),
        ("argument", "class A { public static function f(int $x): int { return $x; } } A::f(\"bad\");", "E0208"),
        ("self_context", "self::f();", "E0268"),
        ("parent_context", "class A { public static function f(): void { parent::f(); } }", "E0268"),
        ("late_binding", "class A { public static function f(): void { static::f(); } }", "E0271"),
        ("abstract_call", "interface I { static function f(): int; } I::f();", "E0269"),
    ];
    for (name, source, code) in cases {
        let f = temp_php(&format!("static_invalid_{name}"), source);
        let r = run_ok(&["check", f.to_str().unwrap()]);
        assert_ne!(r.code, 0, "accepted {name}");
        assert!(r.stderr.contains(code), "{name}: {}", r.stderr);
    }
}

#[test]
fn namespaces_aliases_and_function_fallback_match_php() {
    assert_matches_php(
        "namespaces",
        include_str!("../../../examples/namespaces.php"),
    );
    assert_matches_php(
        "namespace_semicolon",
        r#"<?php
namespace Alpha;
function label(): string { return "alpha"; }
class Item { public static function value(): int { return 4; } }
namespace Beta;
use Alpha\Item;
use function Alpha\label;
echo label() . ":" . Item::value() . ":" . strlen("ok") . "\n";
namespace Gamma;
function strlen(string $value): int { return 99; }
echo strlen("ok") . ":" . \strlen("ok") . "\n";
"#,
    );
}

#[test]
fn module_example_runs_and_all_compiler_commands_resolve_dependencies() {
    if !have_cc() {
        return;
    }
    let f = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/modules.php");
    let r = run_ok(&["run", f.to_str().unwrap()]);
    assert_eq!(r.code, 0, "{}", r.stderr);
    assert_eq!(r.stdout, "[Hello, PXR]\n1\ncontract\n6\n");
    if have_php() {
        let php = Command::new("php").arg(&f).output().unwrap();
        assert!(
            php.status.success(),
            "{}",
            String::from_utf8_lossy(&php.stderr)
        );
        assert_eq!(r.stdout.as_bytes(), php.stdout);
    }
    for command in ["check", "emit-ir", "emit-llvm"] {
        let r = run_ok(&[command, f.to_str().unwrap()]);
        assert_eq!(r.code, 0, "{command}: {}", r.stderr);
    }
}

#[test]
fn module_loader_resolves_transitive_require_once_cycles() {
    if !have_cc() {
        return;
    }
    let main = temp_php(
        "module_graph",
        r#"<?php
require_once __DIR__ . "/graph/a.php";
require_once __DIR__ . "/graph/b.php";
echo \Graph\a() . ":" . \Graph\b() . "\n";
"#,
    );
    let dir = main.parent().unwrap().join("graph");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("a.php"),
        r#"<?php namespace Graph;
require_once __DIR__ . "/b.php";
function a(): int { return 1; }
"#,
    )
    .unwrap();
    std::fs::write(
        dir.join("b.php"),
        r#"<?php namespace Graph;
require_once __DIR__ . "/a.php";
function b(): int { return 2; }
"#,
    )
    .unwrap();
    let r = run_ok(&["run", main.to_str().unwrap()]);
    assert_eq!(r.code, 0, "{}", r.stderr);
    assert_eq!(r.stdout, "1:2\n");
}

#[test]
fn invalid_namespaces_and_module_forms_are_rejected() {
    let cases = [
        ("mixed", "namespace A; namespace B {}", "E0100"),
        ("late", "echo 1; namespace A;", "E0100"),
        ("import_before_namespace", "use A\\B; namespace C;", "E0100"),
        ("nested", "namespace A { namespace B {} }", "E0100"),
        ("outside", "namespace A {} echo 1;", "E0100"),
        ("alias", "use A\\B as X; use C\\D as x;", "E0100"),
        ("alias_declaration", "use A\\B as X; class X {}", "E0100"),
        ("grouped", "use A\\{B, C};", "E0100"),
        ("dynamic", "$file = 'x.php'; require_once $file;", "E0100"),
        (
            "missing",
            "require_once 'pxr_nonexistent_module.php';",
            "E0273",
        ),
        (
            "conditional",
            "if (true) { require_once 'x.php'; }",
            "E0273",
        ),
        ("after_code", "echo 1; require_once 'x.php';", "E0273"),
        (
            "qualified_no_fallback",
            "namespace A; Missing\\strlen('x');",
            "E0232",
        ),
        (
            "relative_no_fallback",
            "namespace A; namespace\\strlen('x');",
            "E0232",
        ),
        (
            "class_no_fallback",
            "namespace A { class X {} } namespace B { $x = new X(); }",
            "E0205",
        ),
    ];
    for (name, source, code) in cases {
        let f = temp_php(&format!("module_invalid_{name}"), source);
        let r = run_ok(&["check", f.to_str().unwrap()]);
        assert_ne!(r.code, 0, "accepted {name}");
        assert!(r.stderr.contains(code), "{name}: {}", r.stderr);
    }
}

#[test]
fn module_diagnostics_name_the_dependency_and_show_its_source() {
    let main = temp_php(
        "module_diagnostics",
        "<?php require_once __DIR__ . '/broken.php';",
    );
    let dependency = main.parent().unwrap().join("broken.php");
    std::fs::write(
        &dependency,
        "<?php\nnamespace Broken;\nfunction fail(): int { return \"bad\"; }\n",
    )
    .unwrap();
    for command in ["check", "build", "emit-ir", "emit-llvm"] {
        let r = run_ok(&[command, main.to_str().unwrap()]);
        assert_ne!(r.code, 0, "{command}");
        assert!(r.stderr.contains("broken.php"), "{}", r.stderr);
        assert!(r.stderr.contains("return \"bad\""), "{}", r.stderr);
        assert!(r.stderr.contains("E0215"), "{}", r.stderr);
    }
    std::fs::write(&dependency, "<?php\n\necho 'side effect';").unwrap();
    let r = run_ok(&["check", main.to_str().unwrap()]);
    assert_ne!(r.code, 0);
    assert!(r.stderr.contains("declarations and dependencies only"));
    assert!(r.stderr.contains("echo 'side effect'"), "{}", r.stderr);
    std::fs::write(
        &dependency,
        "<?php require __DIR__ . '/module_diagnostics.php';",
    )
    .unwrap();
    let r = run_ok(&["check", main.to_str().unwrap()]);
    assert_ne!(r.code, 0);
    assert!(r.stderr.contains("cyclic"), "{}", r.stderr);
}

#[test]
fn mixed_scalars_parameters_returns_and_properties_match_php() {
    if !have_cc() {
        return;
    }
    let src = include_str!("../../../examples/mixed.php");
    let f = temp_php("mixed_scalars", src);
    let r = run_ok(&["run", f.to_str().unwrap()]);
    assert_eq!(r.code, 0, "{}", r.stderr);
    assert_eq!(r.stdout, "int:42\nfloat:3.5\nbool:1\nstring:hello\nboxed\nfalse\n9\nint:1\nstring:two\nbool:\ndifferent\nmatched\n");
    assert_matches_php("mixed_scalars_php", src);
}

#[test]
fn mixed_predicates_and_strict_comparison_match_php() {
    assert_matches_php(
        "mixed_predicates",
        r#"<?php
function boxValue(mixed $value): mixed { return $value; }
function tags(mixed $value): void {
    echo is_int($value) . is_float($value) . is_bool($value) . is_string($value) . ":";
    echo is_scalar($value) . is_array($value) . is_object($value) . is_null($value) . "\n";
}
tags(1); tags(1.5); tags(true); tags("a\0b");
class Plain {}
echo is_array([1]) . is_object(new Plain()) . is_scalar("x") . "\n";
echo (boxValue(1) === 1 ? "yes" : "no") . "\n";
echo (1 === boxValue(1) ? "yes" : "no") . "\n";
echo (boxValue(1) !== boxValue(1.0) ? "yes" : "no") . "\n";
echo (boxValue("a\0b") === boxValue("a\0b") ? "yes" : "no") . "\n";
echo (boxValue(-0.0) === boxValue(0.0) ? "yes" : "no") . "\n";
"#,
    );
}

#[test]
fn mixed_conversions_truthiness_and_control_flow_match_php() {
    assert_matches_php(
        "mixed_control",
        r#"<?php
function boxValue(mixed $value): mixed { return $value; }
function selected(bool $flag, mixed $value): mixed { return $flag ? $value : "fallback"; }
echo selected(true, 1) . ":" . selected(false, 1) . "\n";
echo intval(boxValue(3.9)) . ":" . floatval(boxValue("2.5")) . ":" . strval(boxValue(false)) . "\n";
echo intval(boxValue(1e20)) . ":" . intval(boxValue(-1e20)) . "\n";
echo intval(boxValue(1e100)) . ":" . intval(boxValue(-1.9)) . "\n";
echo intval(boxValue(9223372036854775808.0)) . ":" . intval(boxValue(-9223372036854775808.0)) . "\n";
echo (boxValue("0") ? "yes" : "no") . ":" . (boxValue("\0") ? "yes" : "no") . "\n";
echo (!boxValue(false) && boxValue(1) ? "yes" : "no") . "\n";
echo (boxValue(false) || boxValue("yes") ? "yes" : "no") . "\n";
echo match (boxValue(1)) { "1" => "string", 1.0 => "float", 1 => "int", default => "other" } . "\n";
class Defaults { public mixed $value = "default"; public float $wide = 2; }
$d = new Defaults(); echo $d->value . ":" . $d->wide . "\n";
class WideConstructor {
    public mixed $value = 0;
    public function __construct(mixed $value) { $this->value = $value; }
}
$w = new WideConstructor(4.5); echo $w->value . "\n";
function widened(): float { return 3; }
echo widened() . "\n";
"#,
    );
}

#[test]
fn mixed_opt_in_assignments_and_array_storage_work() {
    if !have_cc() {
        return;
    }
    let f = temp_php(
        "mixed_opt_in",
        r#"<?php
function boxValue(mixed $value): mixed { return $value; }
$value: mixed = 1;
$old = $value;
$value = "next";
echo $old . ":" . $value . "\n";
$value .= "!";
echo $value . "\n";
$values: array<mixed> = [boxValue(1), boxValue("two")];
$values[] = true;
$copy = $values;
$copy[0] = "changed";
$copy[0] .= "!";
foreach ($values as $v) { echo strval($v) . "|"; }
echo "\n" . $copy[0] . "\n";
echo ($values[99] ?? "missing") . "\n";
$value = true ? boxValue(2) : 3;
echo $value . "\n";
"#,
    );
    let r = run_ok(&["run", f.to_str().unwrap()]);
    assert_eq!(r.code, 0, "{}", r.stderr);
    assert_eq!(r.stdout, "1:next\nnext!\n1|two|1|\nchanged!\nmissing\n2\n");
}

#[test]
fn unsupported_mixed_operations_are_rejected() {
    let cases = [
        (
            "arithmetic",
            "function f(mixed $v): mixed { return $v + 1; }",
            "E0274",
        ),
        (
            "loose_compare",
            "function f(mixed $v): bool { return $v == 1; }",
            "E0274",
        ),
        (
            "ordering",
            "function f(mixed $v): bool { return $v < 1; }",
            "E0274",
        ),
        (
            "implicit_unbox",
            "function f(mixed $v): int { return $v; }",
            "E0215",
        ),
        (
            "array_box",
            "function f(mixed $v): void {} f([1]);",
            "E0231",
        ),
        (
            "object_box",
            "class C {} function f(mixed $v): void {} f(new C());",
            "E0231",
        ),
        (
            "whole_array_conversion",
            "$a: array<mixed> = [1, 2];",
            "E0212",
        ),
        (
            "array_property_default",
            "class C { public mixed $value = [1]; }",
            "E0274",
        ),
        ("null", "function f(): mixed { return null; }", "E0209"),
        (
            "builtin",
            "function f(mixed $v): int { return strlen($v); }",
            "E0234",
        ),
        ("predicate_arity", "is_int();", "E0233"),
        (
            "predicate_void",
            "function f(): void {} is_int(f());",
            "E0234",
        ),
    ];
    for (name, source, code) in cases {
        let f = temp_php(&format!("mixed_invalid_{name}"), source);
        let r = run_ok(&["check", f.to_str().unwrap()]);
        assert_ne!(r.code, 0, "accepted {name}");
        assert!(r.stderr.contains(code), "{name}: {}", r.stderr);
    }
}

#[test]
fn mixed_guard_narrowing_and_mutation_match_php() {
    assert_matches_php(
        "mixed_narrowing",
        include_str!("../../../examples/narrowing.php"),
    );
    assert_matches_php(
        "mixed_narrowing_widening",
        r#"<?php
function increase(mixed $value): float {
    if (!is_int($value)) { return 0.0; }
    $value += 0.5;
    return $value;
}
echo increase(3) . "\n";
"#,
    );
}

#[test]
fn mixed_short_circuit_and_ternary_narrowing_match_php() {
    assert_matches_php(
        "mixed_narrowing_expressions",
        r#"<?php
function accept(mixed $value): bool { return !is_int($value) || $value > 2; }
function length(mixed $value): int { return is_string($value) ? strlen($value) : 0; }
function guarded(mixed $value): int {
    if (is_int($value) === false) { return 0; }
    return $value + 1;
}
function literal(mixed $value): int { if ($value === 4) { return $value + 1; } return 0; }
echo accept("x") . ":" . accept(3) . ":" . accept(1) . "\n";
echo length("hello") . ":" . length(false) . "\n";
echo guarded(4) . ":" . guarded("4") . "\n";
echo literal(4) . ":" . literal("4") . "\n";
"#,
    );
}

#[test]
fn mixed_loop_guards_and_continue_narrowing_match_php() {
    assert_matches_php(
        "mixed_narrowing_loops",
        r#"<?php
function boxedLoop(mixed $value): void {
    $i = 0;
    while (is_int($value) && $i < 2) {
        echo $value + 1;
        $value++;
        $i++;
    }
    echo "\n";
}
boxedLoop(2); boxedLoop("x");
function item(mixed $value): void {
    $i = 0;
    while ($i < 2) {
        $i++;
        if (!is_int($value)) { continue; }
        echo $value + 1;
    }
    echo "\n";
}
item(4); item("x");
function forGuard(mixed $value): void {
    $i = 0;
    for (; is_int($value) && $i < 2; $value++) { echo $value; $i++; }
    echo "\n";
}
forGuard(2); forGuard("x");
function reused(mixed $value): void {
    if (!is_string($value)) { return; }
    foreach ([1, 2] as $value) {
        if (is_int($value)) { echo $value + 1; }
    }
    echo "\n";
}
reused("start");
"#,
    );
}

#[test]
fn mixed_narrowing_invalidates_assignments_and_joins() {
    let cases = [
        ("assignment", "function f(mixed $v): int { if (is_int($v)) { $v = 'changed'; return $v + 1; } return 0; }"),
        ("branch_join", "function f(mixed $v, bool $b): int { if (!is_int($v)) { return 0; } if ($b) { $v = 'changed'; } return $v + 1; }"),
        ("loop_backedge", "function f(mixed $v): void { if (is_int($v)) { $i = 0; while ($i < 2) { echo $v + 1; $v = 'changed'; $i++; } } }"),
        ("loop_exit", "function f(mixed $v): int { if (!is_int($v)) { return 0; } while (true) { $v = 'changed'; break; } return $v + 1; }"),
        ("or_does_not_prove", "function f(mixed $v, bool $b): int { if (is_int($v) || $b) { return $v + 1; } return 0; }"),
        ("user_function", "function is_int(mixed $v): bool { return true; } function f(mixed $v): int { if (is_int($v)) { return $v + 1; } return 0; }"),
    ];
    for (name, source) in cases {
        let file = temp_php(&format!("narrowing_invalid_{name}"), source);
        let r = run_ok(&["check", file.to_str().unwrap()]);
        assert_ne!(r.code, 0, "accepted unsound refinement: {name}");
        assert!(r.stderr.contains("E0274"), "{name}: {}", r.stderr);
    }
}
