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
fn interpolation_is_rejected_at_compile_time() {
    // PXR cannot yet expand `"$x"`. Rather than silently emitting the literal
    // (which would diverge from PHP), it is a hard compile error.
    let f = temp_php("interp", "<?php $x = 1; echo \"hi $x\";");
    let r = run_ok(&["check", f.to_str().unwrap()]);
    assert_eq!(r.code, 1);
    assert!(r.stderr.contains("E0008"), "stderr: {}", r.stderr);
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
