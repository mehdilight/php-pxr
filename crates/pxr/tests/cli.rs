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
fn generic_functions_run_with_concrete_types_and_recursion() {
    if !have_cc() {
        return;
    }
    let source = include_str!("../../../examples/generics.php");
    let f = temp_php("generic_functions", source);
    let r = run_ok(&["run", f.to_str().unwrap()]);
    assert_eq!(r.code, 0, "{}", r.stderr);
    assert_eq!(r.stdout, "42:hello\n7:empty\nforwarded:9\n1:99\n8:4\nok\n");
}

#[test]
fn generic_specializations_reuse_concrete_native_signatures() {
    let f = temp_php(
        "generic_ir",
        r#"<?php
        function identity<T>(T $value): T { return $value; }
        function unused<U>(U $value): U { return $value; }
        echo identity(1), identity<int>(2), identity('s');
    "#,
    );
    let r = run_ok(&["emit-ir", f.to_str().unwrap()]);
    assert_eq!(r.code, 0, "{}", r.stderr);
    let definitions: Vec<_> = r
        .stdout
        .lines()
        .filter(|line| line.starts_with("func pxr$g$"))
        .collect();
    assert_eq!(definitions.len(), 2, "{}", r.stdout);
    assert!(definitions
        .iter()
        .any(|line| line.contains("(%v0: int) -> int")));
    assert!(definitions
        .iter()
        .any(|line| line.contains("(%v0: string) -> string")));
    assert!(!r.stdout.contains("mixed"), "{}", r.stdout);
}

#[test]
fn generic_arrays_objects_and_explicit_widening() {
    if !have_cc() {
        return;
    }
    let f = temp_php(
        "generic_values",
        r#"<?php
        class ParentItem { public int $value = 1; }
        class ChildItem extends ParentItem {}
        function identity<T>(T $value): T { return $value; }
        function replace<T>(array<T> $values, T $value): array<T> {
            $values[0] = $value;
            return $values;
        }
        function choose<T>(T $a, T $b): T { return $b; }
        function first<T>(array<T> $values): T { return $values[0]; }
        $original = [[1]];
        $changed = replace($original, [2]);
        echo $original[0][0], ':', $changed[0][0], "\n";
        $object = new ChildItem();
        $alias = identity<ParentItem>($object);
        $alias->value = 7;
        $items = identity<array<ParentItem>>([$object]);
        echo first($items)->value, ':', $object->value, "\n";
        echo choose<float>(1, 2.5), ':', identity(true), "\n";
        $mixed = replace<mixed>([1, 'two'], null);
        echo is_null($mixed[0]), ':', $mixed[1], "\n";
        $empty = identity([]);
        echo count($empty), "\n";
    "#,
    );
    let r = run_ok(&["run", f.to_str().unwrap()]);
    assert_eq!(r.code, 0, "{}", r.stderr);
    assert_eq!(r.stdout, "1:2\n7:7\n2.5:1\n1:two\n0\n");
}

#[test]
fn generic_namespaces_imports_and_mutual_recursion() {
    if !have_cc() {
        return;
    }
    let module = temp_php(
        "generic_module",
        r#"<?php
        namespace Templates;
        function left<T>(T $value, int $n): T {
            if ($n == 0) { return $value; }
            return right<T>($value, $n - 1);
        }
        function right<U>(U $value, int $n): U {
            if ($n == 0) { return $value; }
            return left($value, $n - 1);
        }
    "#,
    );
    let source = format!(
        r#"<?php
        namespace Client;
        require '{}';
        use function Templates\left as run;
        echo run<string>('ok', 5), ':', \Templates\right(42, 4), "\n";
    "#,
        module.display()
    );
    let f = temp_php("generic_modules", &source);
    let r = run_ok(&["run", f.to_str().unwrap()]);
    assert_eq!(r.code, 0, "{}", r.stderr);
    assert_eq!(r.stdout, "ok:42\n");
}

#[test]
fn generic_multiple_parameters_preserve_evaluation_order() {
    if !have_cc() {
        return;
    }
    let f = temp_php(
        "generic_multiple",
        r#"<?php
        class State { public int $calls = 0; }
        function next(State $state): int { $state->calls += 1; return $state->calls; }
        function second<A,B>(A $a, B $b): B { echo $a, ':'; return $b; }
        function show<T>(T $value): void { echo $value, "\n"; }
        $state = new State();
        show(second(next($state), next($state)));
        show(second<int,string>(next($state), 'ok'));
        echo $state->calls, "\n";
    "#,
    );
    let r = run_ok(&["run", f.to_str().unwrap()]);
    assert_eq!(r.code, 0, "{}", r.stderr);
    assert_eq!(r.stdout, "1:2\n3:ok\n3\n");
}

#[test]
fn generic_invalid_programs_fail_with_diagnostics() {
    let cases = [
        (
            "arity",
            "function f<T>(T $x): T {return $x;} f<int,string>(1);",
            "E0280",
        ),
        (
            "values_arity",
            "function f<T>(T $x): T {return $x;} f();",
            "E0230",
        ),
        (
            "void",
            "function f<T>(T $x): T {return $x;} f<void>(1);",
            "E0280",
        ),
        (
            "void_array",
            "function f<T>(T $x): T {return $x;} f<array<void>>([]);",
            "E0205",
        ),
        (
            "unknown",
            "function f<T>(T $x): T {return $x;} f<Missing>(1);",
            "E0205",
        ),
        (
            "conflict",
            "function f<T>(T $x,T $y): T {return $x;} f(1,2.0);",
            "E0281",
        ),
        (
            "empty",
            "function f<T>(array<T> $x): T {return $x[0];} f([]);",
            "E0281",
        ),
        (
            "return_only",
            "function f<T>(): T {return 1;} f();",
            "E0281",
        ),
        (
            "wrong_value",
            "function f<T>(T $x): T {return $x;} f<int>('s');",
            "E0231",
        ),
        (
            "array_widen",
            "function f<T>(array<T> $x): T {return $x[0];} f<float>([1]);",
            "E0231",
        ),
        (
            "ordinary_array_widen",
            "function f(array<float> $x): float {return $x[0];} f([1]);",
            "E0231",
        ),
        (
            "deep_rebox",
            "function f<T>(T $x): T {return $x;} f<array<array<mixed>>>([[1]]);",
            "E0231",
        ),
        (
            "nongeneric",
            "function f(int $x): int {return $x;} f<int>(1);",
            "E0280",
        ),
        ("builtin", "strlen<int>('s');", "E0280"),
        (
            "duplicate_type",
            "function f<T,T>(T $x): T {return $x;}",
            "E0100",
        ),
        (
            "duplicate_value",
            "function f<T>(T $x,T $x): T {return $x;}",
            "E0201",
        ),
        (
            "duplicate_function",
            "function f<T>(T $x): T {return $x;} function F(int $x): int {return $x;}",
            "E0201",
        ),
        (
            "out_of_scope",
            "function f<T>(T $x): T {return $x;} function g(T $x): int {return 1;}",
            "E0205",
        ),
        (
            "unknown_constraint",
            "function f<T: Countable>(T $x): T {return $x;}",
            "E0205",
        ),
    ];
    for (name, source, code) in cases {
        let f = temp_php(&format!("generic_error_{name}"), &format!("<?php {source}"));
        let r = run_ok(&["check", f.to_str().unwrap()]);
        assert_eq!(r.code, 1, "{name}: {}", r.stderr);
        assert!(
            r.stderr.contains(code),
            "{name}: expected {code}: {}",
            r.stderr
        );
    }
}

#[test]
fn generic_body_errors_identify_the_specialization() {
    let f = temp_php(
        "generic_body_error",
        r#"<?php
        function invert<T>(T $value): T { return ~$value; }
        echo invert('s');
    "#,
    );
    let r = run_ok(&["check", f.to_str().unwrap()]);
    assert_eq!(r.code, 1);
    assert!(r.stderr.contains("E0221"), "{}", r.stderr);
    assert!(
        r.stderr.contains("while specializing `invert<string>`"),
        "{}",
        r.stderr
    );
}

#[test]
fn generic_polymorphic_recursion_is_bounded() {
    let f = temp_php(
        "generic_limit",
        r#"<?php
        function grow<T>(T $value): void { grow([$value]); }
        grow(1);
    "#,
    );
    let r = run_ok(&["check", f.to_str().unwrap()]);
    assert_eq!(r.code, 1, "{}", r.stderr);
    assert!(r.stderr.contains("E0283"), "{}", r.stderr);
}

#[test]
fn generic_classes_run_and_compose_with_function_inference() {
    if !have_cc() {
        return;
    }
    let f = temp_php(
        "generic_classes",
        include_str!("../../../examples/generic_classes.php"),
    );
    let r = run_ok(&["run", f.to_str().unwrap()]);
    assert_eq!(r.code, 0, "{}", r.stderr);
    assert_eq!(r.stdout, "42:hello\n7\n1:99\n7\n1:\n");
}

#[test]
fn generic_class_static_state_dispatch_and_interfaces() {
    if !have_cc() {
        return;
    }
    let f = temp_php(
        "generic_class_dispatch",
        r#"<?php
        interface Reader<T> { public function get(): T; }
        class Base<T> implements Reader<T> {
            protected T $value;
            public static int $calls = 0;
            public function __construct(T $value) { $this->value = $value; }
            public function get(): T { self::$calls += 1; return $this->value; }
            public static function count(): int { return self::$calls; }
        }
        class Child<T> extends Base<T> {
            public function get(): T { return parent::get(); }
        }
        class Number extends Child<int> {}
        function read<T>(Reader<T> $reader): T { return $reader->get(); }
        $number = new Number(7);
        $word = new Child<string>('ok');
        echo read($number), ':', $word->get(), "\n";
        Base<int>::$calls += 2;
        echo Base<int>::count(), ':', Base<string>::count(), "\n";
        echo ($number instanceof Base<int>), ':', ($number instanceof Reader<int>), ':', ($number instanceof Reader<string>), "\n";
    "#,
    );
    let r = run_ok(&["run", f.to_str().unwrap()]);
    assert_eq!(r.code, 0, "{}", r.stderr);
    assert_eq!(r.stdout, "7:ok\n3:1\n1:1:\n");
}

#[test]
fn generic_class_aliases_and_nominal_type_arguments() {
    if !have_cc() {
        return;
    }
    let module = temp_php(
        "generic_class_module",
        r#"<?php
        namespace Library;
        class Box<T> {
            public T $value;
            public function __construct(T $value) { $this->value = $value; }
            public function get(): T { return $this->value; }
        }
    "#,
    );
    let source = format!(
        r#"<?php
        namespace App;
        require '{}';
        use Library\Box as Container;
        class Item {{ public string $label = 'item'; }}
        function wrap<T>(T $value): Container<T> {{ return new Container<T>($value); }}
        function unwrap<T>(Container<T> $box): T {{ return $box->get(); }}
        $box: Container<Item> = wrap(new Item());
        $alias = unwrap($box);
        $alias->label = 'shared';
        echo $box->get()->label, "\n";
        $same: \Library\Box<Item> = new Container<item>(new Item());
        echo unwrap($same)->label, "\n";
    "#,
        module.display()
    );
    let f = temp_php("generic_class_aliases", &source);
    let r = run_ok(&["run", f.to_str().unwrap()]);
    assert_eq!(r.code, 0, "{}", r.stderr);
    assert_eq!(r.stdout, "shared\nitem\n");
}

#[test]
fn generic_classes_mixed_and_empty_array_specializations() {
    if !have_cc() {
        return;
    }
    let f = temp_php(
        "generic_class_mixed",
        r#"<?php
        class Box<T> {
            public T $value;
            public function __construct(T $value) { $this->value = $value; }
            public function get(): T { return $this->value; }
            public function __toString(): string { return strval($this->value); }
        }
        function wrap<T>(T $value): Box<T> { return new Box<T>($value); }
        function unwrap<T>(Box<T> $box): T { return $box->get(); }
        $empty = wrap([]);
        echo count(unwrap($empty)), "\n";
        $box = new Box<mixed>([1, 'two']);
        $copy = $box->get();
        if (is_array($copy)) { $copy[0] = 99; }
        $original = $box->get();
        if (is_array($original)) { echo $original[0], ':', $original[1], "\n"; }
        echo wrap(42), ':', wrap('ok'), "\n";
        print wrap('print');
        echo "\n";
    "#,
    );
    let r = run_ok(&["run", f.to_str().unwrap()]);
    assert_eq!(r.code, 0, "{}", r.stderr);
    assert_eq!(r.stdout, "0\n1:two\n42:ok\nprint\n");
}

#[test]
fn generic_class_specializations_are_reused_and_templates_are_lazy() {
    let f = temp_php(
        "generic_class_ir",
        r#"<?php
        class Box<T> { public function get(T $value): T { return $value; } }
        class Unused<T> { public function invalid(): T { return $undefined; } }
        function unused<U>(U $value): U { return new Box<void>(); }
        $one = new Box<int>();
        $two = new box<int>();
        echo $one->get(1), $two->get(2);
    "#,
    );
    let r = run_ok(&["emit-ir", f.to_str().unwrap()]);
    assert_eq!(r.code, 0, "{}", r.stderr);
    assert_eq!(
        r.stdout
            .lines()
            .filter(|line| line.starts_with("func pxr$m$"))
            .count(),
        1,
        "{}",
        r.stdout
    );
}

#[test]
fn generic_class_invalid_programs_fail_loudly() {
    let cases = [
        ("bare_new", "class Box<T> {} new Box();", "E0280"),
        ("bare_type", "class Box<T> {} function f(Box $box): void {}", "E0280"),
        ("arity", "class Box<T> {} new Box<int,string>();", "E0280"),
        ("ordinary", "class Box {} new Box<int>();", "E0280"),
        ("void", "class Box<T> {} new Box<void>();", "E0280"),
        ("void_array", "class Box<T> {} new Box<array<void>>();", "E0205"),
        ("unknown", "class Box<T> {} new Box<Missing>();", "E0205"),
        ("duplicate", "class Box<T,T> {}", "E0100"),
        ("duplicate_class", "class Box<T> {} class box {}", "E0201"),
        ("out_of_scope", "class Box<T> {} function f(T $value): int {return 1;}", "E0205"),
        ("invariant", "class Box<T> {} function f(Box<float> $box): void {} f(new Box<int>());", "E0231"),
        ("constructor", "class Box<T> {public function __construct(T $value) {}} new Box<int>('s');", "E0208"),
        ("field", "class Box<T> {public T $value;} $b = new Box<int>(); $b->value = 's';", "E0212"),
        ("method", "class Box<T> {public function set(T $value): void {}} $b = new Box<int>(); $b->set('s');", "E0208"),
        ("body", "class Box<T> {public function get(): T {return 's';}} new Box<int>();", "E0215"),
        ("override", "class Box<T> {public function get(): T {return 1;}} class Child extends Box<int> {public function get(): string {return 's';}}", "E0208"),
        ("template_signature", "class Box<T> {} function f<U>(Box<U,U> $box): U {return 1;}", "E0280"),
        ("cycle", "class A<T> extends B<T> {} class B<T> extends A<T> {} new A<int>();", "E0201"),
    ];
    for (name, source, code) in cases {
        let f = temp_php(
            &format!("generic_class_error_{name}"),
            &format!("<?php {source}"),
        );
        let r = run_ok(&["check", f.to_str().unwrap()]);
        assert_eq!(r.code, 1, "{name}: {}", r.stderr);
        assert!(
            r.stderr.contains(code),
            "{name}: expected {code}: {}",
            r.stderr
        );
    }
}

#[test]
fn generic_class_expansion_is_bounded() {
    let f = temp_php(
        "generic_class_limit",
        r#"<?php
        class Grow<T> { public Grow<array<T>> $next; }
        new Grow<int>();
    "#,
    );
    let r = run_ok(&["check", f.to_str().unwrap()]);
    assert_eq!(r.code, 1, "{}", r.stderr);
    assert!(r.stderr.contains("E0283"), "{}", r.stderr);
    let f = temp_php(
        "generic_class_exponential_limit",
        r#"<?php
        class Pair<A,B> {}
        class Grow<T> { public Grow<Pair<T,T>> $next; }
        new Grow<int>();
    "#,
    );
    let r = run_ok(&["check", f.to_str().unwrap()]);
    assert_eq!(r.code, 1, "{}", r.stderr);
    assert!(r.stderr.contains("E0283"), "{}", r.stderr);
    assert!(r.stderr.contains("16384-byte"), "{}", r.stderr);
}

#[test]
fn generic_class_multiple_parameters_and_recursive_fields() {
    if !have_cc() {
        return;
    }
    let f = temp_php(
        "generic_class_recursive",
        r#"<?php
        class Pair<A,B> {
            public A $first;
            public B $second;
            public function __construct(A $first, B $second) {
                $this->first = $first; $this->second = $second;
            }
        }
        class Node<T> {
            public T $value;
            public Node<T> $next;
            public function __construct(T $value) { $this->value = $value; }
            public function link(Node<T> $next): void { $this->next = $next; }
        }
        function second<A,B>(Pair<A,B> $pair): B { return $pair->second; }
        $pair = new Pair<int,string>(42, 'ok');
        $head = new Node<array<int>>([1]);
        $tail = new Node<array<int>>([2]);
        $head->link($tail);
        echo second($pair), ':', $head->next->value[0], "\n";
    "#,
    );
    let r = run_ok(&["run", f.to_str().unwrap()]);
    assert_eq!(r.code, 0, "{}", r.stderr);
    assert_eq!(r.stdout, "ok:2\n");
}

#[test]
fn native_typed_object_echo_and_print_use_magic_stringification() {
    assert_matches_php(
        "typed_object_print",
        r#"<?php
        class BaseText { public function __toString(): string { return 'base'; } }
        class ChildText extends BaseText { public function __toString(): string { return 'child'; } }
        function value(): BaseText { return new ChildText(); }
        echo value(), ':';
        print value();
        echo "\n";
    "#,
    );
    if !have_cc() {
        return;
    }
    let f = temp_php(
        "typed_object_no_string",
        "<?php class Item {} echo new Item();",
    );
    let r = run_ok(&["run", f.to_str().unwrap()]);
    assert_eq!(r.code, 255, "{}", r.stderr);
    assert!(
        r.stderr.contains("could not be converted to string"),
        "{}",
        r.stderr
    );
}

#[test]
fn generic_methods_run_with_class_and_method_parameters() {
    if !have_cc() {
        return;
    }
    let f = temp_php(
        "generic_methods",
        include_str!("../../../examples/generic_methods.php"),
    );
    let r = run_ok(&["run", f.to_str().unwrap()]);
    assert_eq!(r.code, 0, "{}", r.stderr);
    assert_eq!(r.stdout, "hello:7\nchanged:42\nstatic:9\ninterface\n");
}

#[test]
fn generic_method_dispatch_handles_interfaces_and_later_classes() {
    if !have_cc() {
        return;
    }
    let f = temp_php(
        "generic_method_dispatch",
        r#"<?php
        interface Contract { public function apply<T>(T $value): T; }
        abstract class Base implements Contract {
            abstract public function apply<U>(U $value): U;
        }
        class Child extends Base {
            public function apply<V>(V $value): V { echo 'child:'; return $value; }
        }
        class Late<T> extends Base {
            public function apply<V>(V $value): V { echo 'late:'; return $value; }
        }
        function throughBase(Base $base): string { return $base->apply<string>('base'); }
        function throughInterface(Contract $value): int { return $value->apply(9); }
        function create<T>(T $unused): Base { return new Late<T>(); }
        echo throughBase(new Child()), "\n";
        echo throughInterface(create(1)), "\n";
        echo throughBase(create('s')), "\n";
    "#,
    );
    let r = run_ok(&["run", f.to_str().unwrap()]);
    assert_eq!(r.code, 0, "{}", r.stderr);
    assert_eq!(r.stdout, "child:base\nlate:9\nlate:base\n");
}

#[test]
fn generic_method_recursion_scope_and_array_value_semantics() {
    if !have_cc() {
        return;
    }
    let f = temp_php(
        "generic_method_values",
        r#"<?php
        class Store<T> {
            private T $value;
            public function __construct(T $value) { $this->value = $value; }
            public function stored<U>(U $unused): T { return $this->value; }
            public function repeat<U>(U $value, int $n): U {
                if ($n == 0) { return $value; }
                return $this->forward<U>($value, $n - 1);
            }
            private function forward<V>(V $value, int $n): V { return $this->repeat<V>($value, $n); }
            public static function choose<A,B>(A $unused, B $value): B { return $value; }
            public static function relay<U>(U $value): U { return self::choose<int,U>(0, $value); }
        }
        $store = new Store<int>(42);
        $original = [[1]];
        $copy = $store->repeat($original, 3);
        $row = $copy[0]; $row[0] = 99; $copy[0] = $row;
        echo $original[0][0], ':', $copy[0][0], ':', $store->stored('s'), "\n";
        echo Store<int>::relay('static'), "\n";
        $mixed = $store->repeat<mixed>([1, 'two'], 1);
        if (is_array($mixed)) { echo $mixed[1], "\n"; }
    "#,
    );
    let r = run_ok(&["run", f.to_str().unwrap()]);
    assert_eq!(r.code, 0, "{}", r.stderr);
    assert_eq!(r.stdout, "1:99:42\nstatic\ntwo\n");
}

#[test]
fn generic_method_receiver_and_arguments_evaluate_once_in_order() {
    if !have_cc() {
        return;
    }
    let f = temp_php(
        "generic_method_order",
        r#"<?php
        class Tools {
            public function second<A,B>(A $a, B $b): B { echo $a, ':'; return $b; }
        }
        function receiver(): Tools { echo 'receiver:'; return new Tools(); }
        function argument(int $value): int { echo $value, ':'; return $value; }
        echo receiver()->second(argument(1), argument(2)), "\n";
    "#,
    );
    let r = run_ok(&["run", f.to_str().unwrap()]);
    assert_eq!(r.code, 0, "{}", r.stderr);
    assert_eq!(r.stdout, "receiver:1:2:1:2\n");
}

#[test]
fn generic_method_specializations_reuse_and_unused_bodies_stay_lazy() {
    let f = temp_php(
        "generic_method_ir",
        r#"<?php
        interface Contract { public function id<T>(T $value): T; }
        class Tools implements Contract {
            public function id<U>(U $value): U { return $value; }
            public function unused<T>(T $value): T { return new Missing(); }
        }
        class Other implements Contract {
            public function id<U>(U $value): U { return $undefined; }
        }
        class StaticTools {public static function id<T>(T $value): T {return $value;}}
        class UnusedStaticChild extends StaticTools {public static function id<U>(U $value): U {return $undefined;}}
        class ParentTools {public function id<T>(T $value): T {return $value;}}
        class ParentCaller extends ParentTools {public function run(): int {return parent::id<int>(1);}}
        class UnusedSibling extends ParentTools {public function id<U>(U $value): U {return $undefined;}}
        $tools = new Tools();
        echo $tools->id(1), $tools->id<int>(2);
        echo StaticTools::id(3);
    "#,
    );
    let r = run_ok(&["emit-ir", f.to_str().unwrap()]);
    assert_eq!(r.code, 0, "{}", r.stderr);
    let definitions: Vec<_> = r
        .stdout
        .lines()
        .filter(|line| line.starts_with("func pxr$m$"))
        .collect();
    assert_eq!(definitions.len(), 4, "{}", r.stdout);
    assert_eq!(
        definitions
            .iter()
            .filter(|line| line.contains("(%v0: Tools, %v1: int) -> int"))
            .count(),
        1,
        "{}",
        r.stdout
    );
}

#[test]
fn generic_method_diagnostics_reject_invalid_programs() {
    let cases = [
        ("type_arity", "class C {public function f<T>(T $v): T {return $v;}} (new C())->f<int,string>(1);", "E0280"),
        ("value_arity", "class C {public function f<T>(T $v): T {return $v;}} (new C())->f();", "E0230"),
        ("missing_inference", "class C {public function f<T>(array<T> $v): T {return $v[0];}} (new C())->f([]);", "E0281"),
        ("conflict", "class C {public function f<T>(T $a,T $b): T {return $a;}} (new C())->f(1,'s');", "E0281"),
        ("void", "class C {public static function f<T>(T $v): T {return $v;}} C::f<void>(1);", "E0280"),
        ("ordinary", "class C {public function f(int $v): int {return $v;}} (new C())->f<int>(1);", "E0280"),
        ("ordinary_static", "class C {public static function f(int $v): int {return $v;}} C::f<int>(1);", "E0280"),
        ("wrong_value", "class C {public function f<T>(T $v): T {return $v;}} (new C())->f<int>('s');", "E0231"),
        ("shadow", "class C<T> {public function f<T>(T $v): T {return $v;}}", "E0100"),
        ("duplicate", "class C {public function f<T,T>(T $v): T {return $v;}}", "E0100"),
        ("duplicate_value", "class C {public function f<T>(T $v,T $v): T {return $v;}}", "E0201"),
        ("this_parameter", "class C {public function f<T>(T $this): T {return $this;}}", "E0267"),
        ("scope", "class C {public function f<T>(T $v): T {return $v;} public function g(T $v): int {return 1;}}", "E0205"),
        ("override_arity", "class B {public function f<T>(T $v): T {return $v;}} class C extends B {public function f<A,B>(A $v): A {return $v;}}", "E0282"),
        ("override_shape", "class B {public function f<A,B>(A $v,B $w): A {return $v;}} class C extends B {public function f<X,Y>(Y $v,X $w): Y {return $v;}}", "E0282"),
        ("interface_shape", "interface I {public function f<T>(T $v): T;} class C implements I {public function f<T>(array<T> $v): T {return $v[0];}}", "E0282"),
        ("erase_generic", "interface I {public function f<T>(T $v): T;} class C implements I {public function f(mixed $v): mixed {return $v;}}", "E0282"),
        ("constructor", "class C {public function __construct<T>(T $v) {}}", "E0282"),
        ("magic", "class C {public function __toString<T>(): string {return 's';}}", "E0282"),
        ("private", "class C {private function f<T>(T $v): T {return $v;}} (new C())->f(1);", "E0265"),
        ("static_object", "class C {public static function f<T>(T $v): T {return $v;}} (new C())->f(1);", "E0271"),
        ("no_receiver", "class C {public function f<T>(T $v): T {return $v;}} C::f(1);", "E0271"),
    ];
    for (name, source, code) in cases {
        let f = temp_php(
            &format!("generic_method_error_{name}"),
            &format!("<?php {source}"),
        );
        let r = run_ok(&["check", f.to_str().unwrap()]);
        assert_eq!(r.code, 1, "{name}: {}", r.stderr);
        assert!(
            r.stderr.contains(code),
            "{name}: expected {code}: {}",
            r.stderr
        );
    }
}

#[test]
fn generic_method_body_errors_name_the_specialization() {
    let f = temp_php(
        "generic_method_body_error",
        r#"<?php
        class C { public function invert<T>(T $value): T { return ~$value; } }
        echo (new C())->invert('s');
    "#,
    );
    let r = run_ok(&["check", f.to_str().unwrap()]);
    assert_eq!(r.code, 1);
    assert!(r.stderr.contains("E0221"), "{}", r.stderr);
    assert!(
        r.stderr.contains("while specializing `C::invert<string>`"),
        "{}",
        r.stderr
    );
}

#[test]
fn generic_methods_resolve_module_aliases_and_generic_class_patterns() {
    if !have_cc() {
        return;
    }
    let module = temp_php(
        "generic_method_module",
        r#"<?php
        namespace Library;
        class Box<T> {
            public T $value;
            public function __construct(T $value) { $this->value = $value; }
        }
        class Tools<T> {
            public function unwrap<U>(Box<U> $box): U { return $box->value; }
            public static function wrap<U>(U $value): Box<U> { return new Box<U>($value); }
        }
    "#,
    );
    let source = format!(
        r#"<?php
        namespace App;
        require '{}';
        use Library\Tools as Helper;
        $tools = new Helper<array<int>>();
        $box = Helper<array<int>>::wrap<string>('ok');
        echo $tools->unwrap($box), ':', $tools->unwrap<int>(Helper<int>::wrap(7)), "\n";
    "#,
        module.display()
    );
    let f = temp_php("generic_method_modules", &source);
    let r = run_ok(&["run", f.to_str().unwrap()]);
    assert_eq!(r.code, 0, "{}", r.stderr);
    assert_eq!(r.stdout, "ok:7\n");
}

#[test]
fn generic_method_polymorphic_recursion_is_bounded() {
    let f = temp_php(
        "generic_method_limit",
        r#"<?php
        class C {public function grow<T>(T $value): void { $this->grow([$value]); }}
        (new C())->grow(1);
    "#,
    );
    let r = run_ok(&["check", f.to_str().unwrap()]);
    assert_eq!(r.code, 1, "{}", r.stderr);
    assert!(r.stderr.contains("E0283"), "{}", r.stderr);
}

#[test]
fn generic_bounds_run_for_functions_classes_and_methods() {
    if !have_cc() {
        return;
    }
    let f = temp_php(
        "generic_bounds",
        include_str!("../../../examples/constraints.php"),
    );
    let r = run_ok(&["run", f.to_str().unwrap()]);
    assert_eq!(r.code, 0, "{}", r.stderr);
    assert_eq!(r.stdout, "named:item:named:item\nitem\n");
}

#[test]
fn generic_bounds_support_dependent_interfaces_and_module_aliases() {
    if !have_cc() {
        return;
    }
    let module = temp_php(
        "bounds_module",
        r#"<?php
        namespace Contracts;
        interface Reader<T> {public function get(): T;}
        class IntReader implements Reader<int> {public function get(): int {return 42;}}
    "#,
    );
    let source = format!(
        r#"<?php
        namespace App;
        require '{}';
        use Contracts\Reader as Readable;
        use Contracts\IntReader;
        function read<T,U: Readable<T>>(U $reader) {{ return $reader->get(); }}
        function forward<U: Readable<T>,T>(U $reader) {{return $reader->get();}}
        function subtype<A,B:A>(A $a,B $b) {{return $b->get();}}
        class Holder<T,U: Readable<T>> {{
            private U $reader;
            public function __construct(U $reader) {{$this->reader = $reader;}}
            public function get() {{return $this->reader->get();}}
        }}
        echo read<int,IntReader>(new IntReader()), ':', (new Holder<int,IntReader>(new IntReader()))->get(), ':', forward<IntReader,int>(new IntReader()), ':', subtype(new IntReader(),new IntReader()), "\n";
    "#,
        module.display()
    );
    let f = temp_php("dependent_bounds", &source);
    let r = run_ok(&["run", f.to_str().unwrap()]);
    assert_eq!(r.code, 0, "{}", r.stderr);
    assert_eq!(r.stdout, "42:42:42:42\n");
}

#[test]
fn generic_bounds_fail_before_native_generation() {
    let cases = [
        ("scalar", "interface I {} function f<T:I>(T $x): T {return $x;} f(1);", "E0286"),
        ("wrong_object", "interface I {} class C {} function f<T:I>(T $x): T {return $x;} f(new C());", "E0286"),
        ("intersection", "interface I {} interface J {} class C implements I {} function f<T:I & J>(T $x): T {return $x;} f(new C());", "E0286"),
        ("class_bound", "interface I {} class C<T:I> {} new C<int>();", "E0286"),
        ("method_bound", "interface I {} class C {public function f<T:I>(T $v): T {return $v;}} (new C())->f(1);", "E0286"),
        ("invalid_bound", "function f<T:int>(T $v): T {return $v;}", "E0286"),
        ("unknown_bound", "function f<T:Missing>(T $v): T {return $v;}", "E0205"),
        ("wrong_generic_bound", "interface I<T> {} class C implements I<int> {} function f<T:I<string>>(T $v): T {return $v;} f(new C());", "E0286"),
        ("override_strengthening", "interface I {} class B {public function f<T>(T $v): T {return $v;}} class C extends B {public function f<U:I>(U $v): U {return $v;}}", "E0282"),
        ("interface_weakening", "interface I {} interface J {public function f<T:I>(T $v): T;} class C implements J {public function f<U>(U $v): U {return $v;}}", "E0282"),
    ];
    for (name, source, code) in cases {
        let f = temp_php(&format!("bound_error_{name}"), &format!("<?php {source}"));
        let r = run_ok(&["emit-llvm", f.to_str().unwrap()]);
        assert_eq!(r.code, 1, "{name}: {}", r.stderr);
        assert!(
            r.stderr.contains(code),
            "{name}: expected {code}: {}",
            r.stderr
        );
        assert!(r.stdout.is_empty(), "{name}: invalid bounds emitted LLVM");
    }
}

#[test]
fn trait_composition_and_precedence_match_php() {
    assert_matches_php(
        "trait_example",
        include_str!("../../../examples/traits.php"),
    );
    assert_matches_php(
        "trait_precedence",
        r#"<?php
        trait Label {
            private string $label = 'trait';
            public function text(): string {return $this->label;}
            public function method(): string {return 'trait';}
        }
        trait Other {public function method(): string {return 'other';}}
        class ParentItem {public function method(): string {return 'parent';}}
        class Item extends ParentItem {use Label;}
        class Own extends ParentItem {use Label, Other; public function method(): string {return 'own';}}
        echo (new Item())->method(), ':', (new Item())->text(), ':', (new Own())->method(), "\n";
    "#,
    );
}

#[test]
fn trait_generic_methods_bounds_and_return_inference_compose() {
    if !have_cc() {
        return;
    }
    let f = temp_php(
        "trait_generics",
        r#"<?php
        interface Printable {public function text(): string;}
        trait Identity {
            public function identity<U: Printable>(U $value) {return $value;}
            public function fluent() {return $this;}
        }
        class Box<T> implements Printable {
            use Identity;
            private T $value;
            public function __construct(T $value) {$this->value = $value;}
            public function text(): string {return strval($this->value);}
        }
        $box = new Box<int>(42);
        echo $box->fluent()->identity(new Box<string>('hello'))->text(), ':', $box->text(), "\n";
    "#,
    );
    let r = run_ok(&["run", f.to_str().unwrap()]);
    assert_eq!(r.code, 0, "{}", r.stderr);
    assert_eq!(r.stdout, "hello:42\n");
}

#[test]
fn traits_resolve_namespace_aliases_in_modules() {
    if !have_cc() {
        return;
    }
    let module = temp_php(
        "trait_module",
        "<?php namespace Library; trait Label {public function text(): string {return 'module';}}",
    );
    let source = format!(
        r#"<?php namespace App; require '{}'; use Library\Label as Named; class Item {{use Named;}} echo (new Item())->text(), "\n";"#,
        module.display()
    );
    let f = temp_php("trait_module_entry", &source);
    let r = run_ok(&["run", f.to_str().unwrap()]);
    assert_eq!(r.code, 0, "{}", r.stderr);
    assert_eq!(r.stdout, "module\n");
}

#[test]
fn trait_conflicts_and_unsupported_adaptations_fail_loudly() {
    let cases = [
        ("missing", "class C {use Missing;}", "E0285"),
        ("not_trait", "class A {} class C {use A;}", "E0285"),
        ("cycle", "trait A {use B;} trait B {use A;}", "E0285"),
        ("conflict", "trait A {public function f(): int {return 1;}} trait B {public function f(): int {return 2;}} class C {use A,B;}", "E0285"),
        ("properties", "trait A {public int $x=1;} class C {use A; public int $x=1;}", "E0285"),
        ("abstract", "trait A {abstract public function f(): int;}", "E0285"),
        ("interface", "trait A {} interface I {use A;}", "E0285"),
        ("generic_trait", "trait A<T> {}", "E0285"),
        ("trait_argument", "trait A {} class C {use A<int>;}", "E0285"),
        ("adaptation", "trait A {} class C {use A {f as g;}}", "E0100"),
        ("instantiate", "trait A {} new A();", "E0205"),
    ];
    for (name, source, code) in cases {
        let f = temp_php(&format!("trait_error_{name}"), &format!("<?php {source}"));
        let r = run_ok(&["check", f.to_str().unwrap()]);
        assert_eq!(r.code, 1, "{name}: {}", r.stderr);
        assert!(
            r.stderr.contains(code),
            "{name}: expected {code}: {}",
            r.stderr
        );
    }
    let mut source = "<?php trait T0 {}".to_string();
    for n in 1..70 {
        source.push_str(&format!("trait T{n} {{use T{};}}", n - 1));
    }
    source.push_str("class C {use T69;}");
    let f = temp_php("trait_expansion_limit", &source);
    let r = run_ok(&["check", f.to_str().unwrap()]);
    assert_eq!(r.code, 1, "{}", r.stderr);
    assert!(
        r.stderr.contains("E0285") && r.stderr.contains("limit"),
        "{}",
        r.stderr
    );
}

#[test]
fn whole_function_return_inference_runs_generic_and_recursive_code() {
    if !have_cc() {
        return;
    }
    let f = temp_php(
        "return_inference",
        include_str!("../../../examples/inference.php"),
    );
    let r = run_ok(&["run", f.to_str().unwrap()]);
    assert_eq!(r.code, 0, "{}", r.stderr);
    assert_eq!(r.stdout, "42:1:2.5:21\nhello:7\nmethod\n");
}

#[test]
fn return_inference_across_mutual_calls_and_methods_matches_php() {
    assert_matches_php(
        "inferred_returns_php",
        r#"<?php
        function even(int $n) {if ($n == 0) {return true;} return odd($n-1);}
        function odd(int $n) {if ($n == 0) {return false;} return even($n-1);}
        function forward(int $n) {return later($n);}
        function later(int $n) {return $n * 2;}
        function logValue(string $v) {echo $v;}
        class C {
            public function run(int $n) {return $this->next($n);}
            private function next(int $n) {return forward($n);}
            public function __toString() {return 'object';}
        }
        echo even(4), ':', odd(4), ':', (new C())->run(21), ':', new C(), "\n";
        logValue('void'); echo "\n";
    "#,
    );
}

#[test]
fn return_inference_preserves_arrays_mixed_values_and_common_bases() {
    if !have_cc() {
        return;
    }
    let f = temp_php(
        "inferred_values",
        r#"<?php
        function values(bool $empty) {if ($empty) {return [];} return [[1]];}
        function nullable(bool $empty) {if ($empty) {return null;} return 7;}
        class Base {public function name(): string {return 'base';}}
        class Left extends Base {public function name(): string {return 'left';}}
        class Right extends Base {public function name(): string {return 'right';}}
        function object(bool $left) {if ($left) {return new Left();} return new Right();}
        $a = values(false); $b = $a; $row = $b[0]; $row[0] = 99; $b[0] = $row;
        echo $a[0][0], ':', $b[0][0], ':', count(values(true)), "\n";
        echo nullable(false), ':', is_null(nullable(true)), ':', object(false)->name(), "\n";
    "#,
    );
    let r = run_ok(&["run", f.to_str().unwrap()]);
    assert_eq!(r.code, 0, "{}", r.stderr);
    assert_eq!(r.stdout, "1:99:0\n7:1:right\n");
}

#[test]
fn inferred_method_returns_obey_abstract_and_generic_contracts() {
    if !have_cc() {
        return;
    }
    let f = temp_php(
        "inferred_contracts",
        r#"<?php
        interface Contract {public function value(): int; public function id<T>(T $v): T;}
        class C implements Contract {
            public function value() {return 42;}
            public function id<U>(U $v) {return $v;}
        }
        function run(Contract $c) {return $c->id($c->value());}
        echo run(new C()), "\n";
    "#,
    );
    let r = run_ok(&["run", f.to_str().unwrap()]);
    assert_eq!(r.code, 0, "{}", r.stderr);
    assert_eq!(r.stdout, "42\n");
}

#[test]
fn inferred_covariant_returns_wait_for_callee_types_to_settle() {
    if !have_cc() {
        return;
    }
    let f = temp_php(
        "inferred_delayed_covariance",
        r#"<?php
        class Root {}
        class Left extends Root {}
        class Right extends Root {}
        class Base {
            public function value(bool $left) {if ($left) {return new Left();} return later();}
        }
        class Child extends Base {public function value(bool $left) {return new Right();}}
        function later() {return new Right();}
        function throughBase(Base $base) {return $base->value(false);}
        echo (throughBase(new Child()) instanceof Right), "\n";
    "#,
    );
    let r = run_ok(&["run", f.to_str().unwrap()]);
    assert_eq!(r.code, 0, "{}", r.stderr);
    assert_eq!(r.stdout, "1\n");
}

#[test]
fn inferred_widening_discards_obsolete_generic_class_demands() {
    if !have_cc() {
        return;
    }
    let f = temp_php(
        "inferred_stale_specializations",
        r#"<?php
        class Root {}
        class Child extends Root {}
        class Box<T> {public function create(): T {return new Root();}}
        function chosen(bool $child) {if ($child) {return new Child();} return root();}
        function root() {return new Root();}
        function box<T>(T $unused) {return new Box<T>();}
        function create() {return box(chosen(false))->create();}
        echo (create() instanceof Root), "\n";
    "#,
    );
    let r = run_ok(&["run", f.to_str().unwrap()]);
    assert_eq!(r.code, 0, "{}", r.stderr);
    assert_eq!(r.stdout, "1\n");
}

#[test]
fn inferred_native_signatures_are_concrete() {
    let f = temp_php(
        "inferred_ir",
        r#"<?php
        function integer(int $n) {return $n * 2;}
        function number(bool $b) {if ($b) {return 1;} return 2.5;}
        function procedure() {echo 'ok';}
        function overwrite() {$value = overwrite(); $value = 1; return $value;}
        echo integer(2), number(false); procedure();
    "#,
    );
    let r = run_ok(&["emit-ir", f.to_str().unwrap()]);
    assert_eq!(r.code, 0, "{}", r.stderr);
    assert!(
        r.stdout.contains("func pxr$fn$integer(%v0: int) -> int"),
        "{}",
        r.stdout
    );
    assert!(
        r.stdout.contains("func pxr$fn$number(%v0: bool) -> float"),
        "{}",
        r.stdout
    );
    assert!(
        r.stdout.contains("func pxr$fn$procedure() -> void"),
        "{}",
        r.stdout
    );
    assert!(!r.stdout.contains("unresolved"), "{}", r.stdout);
}

#[test]
fn ambiguous_or_invalid_return_inference_never_emits_llvm() {
    let cases = [
        ("cycle", "function f() {return f();} f();", "E0284"),
        ("mutual_cycle", "function f() {return g();} function g() {return f();} f();", "E0284"),
        ("growing", "function f() {return [f()];} f();", "E0284"),
        ("mixed_returns", "function f(bool $b) {if ($b) {return 1;} return 's';} f(true);", "E0284"),
        ("bare_and_value", "function f(bool $b) {if ($b) {return;} return 1;} f(true);", "E0284"),
        ("fallthrough", "function f(bool $b) {if ($b) {return 1;}} f(true);", "E0206"),
        ("array_widen", "function f(bool $b) {if ($b) {return [1];} return [2.5];} f(true);", "E0284"),
        ("void_value", "function f() {} $x = f();", "E0211"),
        ("bad_parameter", "function f(int $x) {return $x;} f('s');", "E0231"),
        ("contract", "interface I {public function f(): int;} class C implements I {public function f() {return 's';}}", "E0208"),
        ("abstract", "interface I {public function f();}", "E0204"),
        ("numeric_override", "class B {public function f() {return 2.5;}} class C extends B {public function f() {return 1;}}", "E0208"),
        ("generic_body", "function f<T>(T $v) {return ~$v;} f('s');", "E0221"),
    ];
    for (name, source, code) in cases {
        let f = temp_php(
            &format!("inference_error_{name}"),
            &format!("<?php {source}"),
        );
        let r = run_ok(&["emit-llvm", f.to_str().unwrap()]);
        assert_eq!(r.code, 1, "{name}: {}", r.stderr);
        assert!(
            r.stderr.contains(code),
            "{name}: expected {code}: {}",
            r.stderr
        );
        assert!(
            r.stdout.is_empty(),
            "{name}: emitted LLVM with incomplete types"
        );
    }
}

#[test]
fn m5_acceptance_builds_optimized_without_php() {
    if !have_cc() {
        return;
    }
    let f = temp_php(
        "m5_acceptance",
        r#"<?php
        interface Named {public function name(): string;}
        trait Identity {public function identity<T: Named>(T $value) {return $value;}}
        class Item implements Named {public function name(): string {return 'native';}}
        class Box<T: Named> {
            use Identity;
            private T $value;
            public function __construct(T $value) {$this->value = $value;}
            public function get() {return $this->identity($this->value);}
        }
        function box<T: Named>(T $value) {return new Box<T>($value);}
        echo box(new Item())->get()->name(), "\n";
    "#,
    );
    let binary = f.with_extension("bin");
    let r = run_ok(&[
        "build",
        f.to_str().unwrap(),
        "--opt",
        "3",
        "-o",
        binary.to_str().unwrap(),
    ]);
    assert_eq!(r.code, 0, "{}", r.stderr);
    let output = Command::new(&binary)
        .env("PXR_PHP", "/missing/php-executable")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(output.stdout, b"native\n");
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
            "implicit_unbox",
            "function f(mixed $v): int { return $v; }",
            "E0215",
        ),
        (
            "null_into_concrete_return",
            "function f(): int { return null; }",
            "E0215",
        ),
        ("null_into_concrete_variable", "$v = 1; $v = null;", "E0212"),
        (
            "invalid_stringifier_return",
            "class C { public function __toString(): int { return 1; } }",
            "E0275",
        ),
        (
            "invalid_stringifier_arguments",
            "class C { public function __toString(int $v): string { return strval($v); } }",
            "E0275",
        ),
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
        ("loop_backedge", "function requireInteger(int $v): void {} function f(mixed $v): void { if (is_int($v)) { $i = 0; while ($i < 2) { requireInteger($v); $v = 'changed'; $i++; } } }"),
        ("loop_exit", "function f(mixed $v): int { if (!is_int($v)) { return 0; } while (true) { $v = 'changed'; break; } return $v + 1; }"),
        ("or_does_not_prove", "function f(mixed $v, bool $b): int { if (is_int($v) || $b) { return $v + 1; } return 0; }"),
        ("user_function", "function is_int(mixed $v): bool { return true; } function f(mixed $v): int { if (is_int($v)) { return $v + 1; } return 0; }"),
    ];
    for (name, source) in cases {
        let file = temp_php(&format!("narrowing_invalid_{name}"), source);
        let r = run_ok(&["check", file.to_str().unwrap()]);
        assert_ne!(r.code, 0, "accepted unsound refinement: {name}");
        assert!(
            r.stderr.contains("E0215") || r.stderr.contains("E0231"),
            "{name}: {}",
            r.stderr
        );
    }
}

#[test]
fn boxed_nulls_and_coalescing_match_php() {
    assert_matches_php("boxed_nulls", include_str!("../../../examples/nulls.php"));
    assert_matches_php(
        "null_coalesce_evaluation",
        r#"<?php
class Holder { public mixed $value = null; public int $number = 1; }
function receiver(Holder $h): Holder { echo "receiver|"; return $h; }
function nextKey(): string { echo "key|"; return "x"; }
function fallback(): mixed { echo "rhs|"; return 9; }
$h = new Holder();
receiver($h)->value ??= fallback();
receiver($h)->value ??= fallback();
receiver($h)->number ??= 5;
echo "\n";
$a = ["x" => null];
$a[nextKey()] ??= fallback();
$a[nextKey()] ??= fallback();
echo "\n" . ($a[nextKey()] ?? fallback()) . "\n";
$a["x"] = null;
echo ($a[nextKey()] ?? fallback()) . "\n";
echo (null ?? null ?? "last") . "\n";
"#,
    );
}

#[test]
fn null_guards_preserve_sound_scalar_narrowing() {
    assert_matches_php(
        "null_guard_paths",
        r#"<?php
function booleanOnly(mixed $v): bool {
    if (is_null($v) || is_int($v) || is_float($v) || is_string($v) || is_array($v) || is_object($v)) { return false; }
    return $v;
}
function surviving(mixed $v): string {
    if ($v === null) { return "null"; }
    if (is_int($v)) { return "int:" . ($v + 1); }
    if (is_float($v) || is_bool($v)) { return "other"; }
    if (is_string($v)) { return strtoupper($v); }
    return "other";
}
function inverse(mixed $v): string {
    if (null !== $v && is_string($v)) { return strtoupper($v); }
    return "other";
}
echo booleanOnly(null) . ":" . booleanOnly(true) . "\n";
echo surviving(null) . ":" . surviving("hello") . ":" . surviving(3) . "\n";
echo inverse(null) . ":" . inverse("yes") . "\n";
"#,
    );
    let f = temp_php(
        "null_prevents_false_exhaustiveness",
        r#"function invalid(mixed $v): bool {
            if (is_int($v) || is_float($v) || is_string($v)) { return false; }
            return $v;
        }"#,
    );
    let r = run_ok(&["check", f.to_str().unwrap()]);
    assert_ne!(r.code, 0);
    assert!(r.stderr.contains("E0215"), "{}", r.stderr);
}

#[test]
fn mixed_arrays_and_objects_preserve_php_value_semantics() {
    assert_matches_php(
        "mixed_containers",
        r#"<?php
class Counter { public int $n = 1; public mixed $defaults = [1, 2]; public static mixed $shared = []; }
function boxContainer(mixed $value): mixed { return $value; }
function changeContainer(mixed $value): mixed {
    if (is_array($value)) { $value[0] = 9; $value[] = "extra"; }
    return $value;
}
function describeContainer(mixed $value): string {
    if (is_array($value)) {
        $text = "array:" . count($value) . ":";
        foreach ($value as $item) { $text .= is_array($item) ? "nested" : strval($item); }
        return $text;
    }
    if ($value instanceof Counter) { $value->n++; return "counter:" . $value->n; }
    return "other";
}
$typed = [1, 2];
$boxed = boxContainer($typed);
$typed[0] = 7;
$changed = changeContainer($boxed);
echo describeContainer($boxed) . "|" . describeContainer($changed) . "\n";
$copy = $boxed;
if (is_array($copy)) { $copy[0] = "changed"; }
echo describeContainer($boxed) . "|" . describeContainer($copy) . "\n";
$nested = boxContainer([[1, 2], [3]]);
if (is_array($nested)) {
    $inner = $nested[0];
    if (is_array($inner)) { $inner[0] = 20; }
    echo describeContainer($nested[0]) . "|" . describeContainer($inner) . "\n";
}
$object = new Counter();
$reference = boxContainer($object);
echo describeContainer($reference) . ":" . $object->n . "\n";
echo is_object($reference) . ":" . is_scalar($reference) . ":" . ($reference === $object) . "\n";
echo ($boxed === boxContainer([1, 2])) . ":" . ($boxed === boxContainer([1, 3])) . "\n";
echo boolval(boxContainer([])) . ":" . boolval($boxed) . ":" . intval($boxed) . ":" . intval([]) . ":" . floatval([1]) . "\n";
echo describeContainer($object->defaults) . "\n";
$object->defaults = ['a' => 1, 'b' => 'two'];
Counter::$shared = ['a' => 3, 'b' => 'four'];
echo describeContainer($object->defaults) . ':' . describeContainer(Counter::$shared) . "\n";
$collection = [boxContainer(0)]; $collection[] = ['a' => 1, 'b' => 'two'];
echo describeContainer($collection[1]) . "\n";
$merged = array_merge([1], [boxContainer('two')]);
foreach ($merged as $item) { echo $item . '|'; }
echo "\n";
"#,
    );
    let f = temp_php(
        "whole_array_to_mixed",
        r#"$a: array<mixed> = [1, 2]; $a[] = "three"; foreach ($a as $v) { echo $v . "|"; }"#,
    );
    let r = run_ok(&["run", f.to_str().unwrap()]);
    assert_eq!(r.code, 0, "{}", r.stderr);
    assert_eq!(r.stdout, "1|2|three|");
}

#[test]
fn mixed_array_transforms_copy_nested_values() {
    assert_matches_php(
        "mixed_array_transforms",
        r#"<?php
function boxedArray(mixed $v): mixed { return $v; }
function mutateFirst(mixed $v): string {
    if (is_array($v)) { $v[0] = 99; return strval($v[0]); }
    return "other";
}
function firstValue(mixed $v): string {
    if (is_array($v)) { return strval($v[0]); }
    return "other";
}
$a = [boxedArray([1, 2]), boxedArray([3])];
$b = array_reverse(array_values($a));
$c = array_slice(array_merge($a, $b), 0, 1);
$d = array_pad($c, 2, boxedArray([4]));
$e = array_fill(0, 2, boxedArray([5]));
echo mutateFirst($b[0]) . ":" . firstValue($a[1]) . ":" . firstValue($b[0]) . "\n";
echo firstValue($c[0]) . ":" . firstValue($d[1]) . ":" . firstValue($e[1]) . "\n";
"#,
    );
}

#[test]
fn mixed_arithmetic_comparison_and_bitwise_match_php() {
    assert_matches_php(
        "mixed_operators",
        r#"<?php
function dynamicValue(mixed $v): mixed { return $v; }
function resultType(mixed $v): string { return is_int($v) ? 'int:' . $v : (is_float($v) ? 'float:' . $v : strval($v)); }
function operations(mixed $a, mixed $b): void {
    echo resultType($a + $b) . '|' . resultType($a - $b) . '|' . resultType($a * $b) . '|' . resultType($a / $b) . "\n";
}
operations(6, 2); operations('6', '2'); operations(5, 2); operations(1.5, 2); operations(null, true);
echo resultType(dynamicValue(9223372036854775807) + 1) . "\n";
echo resultType(dynamicValue(9223372036854775807) * 2) . "\n";
echo resultType(-dynamicValue('12')) . ':' . resultType(~dynamicValue(4)) . "\n";
$flipped = ~dynamicValue('AZ'); if (is_string($flipped)) { echo bin2hex($flipped) . "\n"; }
echo resultType(dynamicValue(7) % '3') . ':' . resultType(dynamicValue(6) & 3) . ':' . resultType(dynamicValue(1) << 3) . "\n";
$bits = dynamicValue('AZ') ^ 'BC'; if (is_string($bits)) { echo bin2hex($bits) . "\n"; }
$v = dynamicValue(2); $v += '3'; $v *= 2; echo $v . "\n";
$a = dynamicValue(['a' => 1]); $b = dynamicValue(['a' => 2, 'b' => 3]);
$union = $a + $b;
if (is_array($union)) { echo $union['a'] . ':' . $union['b'] . "\n"; }
$values = [dynamicValue(null), dynamicValue(false), dynamicValue(true), dynamicValue(0), dynamicValue(1), dynamicValue(-1), dynamicValue(0.5), dynamicValue(''), dynamicValue('0'), dynamicValue('01'), dynamicValue('a'), dynamicValue([]), dynamicValue([1]), dynamicValue(['a' => 1]), dynamicValue(['b' => 1])];
foreach ($values as $x) {
    foreach ($values as $y) { echo ($x == $y) . ':' . ($x != $y) . ':' . ($x < $y) . ':' . ($x <= $y) . ':' . ($x > $y) . ':' . ($x >= $y) . ':' . ($x <=> $y) . '|'; }
    echo "\n";
}
class Pair { public int $x = 1; }
$left = dynamicValue(new Pair()); $right = dynamicValue(new Pair());
echo ($left == $right) . ':' . ($left === $right) . ':' . ($left <=> $right) . "\n";
switch (dynamicValue('01')) { case 1: echo "loose switch\n"; break; default: echo "wrong\n"; }
echo ([1, 2] === [1, 2]) . ':' . (new Pair() === new Pair()) . "\n";
"#,
    );
}

#[test]
fn mixed_runtime_operand_errors_fail_loudly() {
    for (name, source, message) in [
        (
            "numeric",
            "function box(mixed $v): mixed {return $v;} echo box('oops') + 1;",
            "Unsupported operand types",
        ),
        (
            "array_numeric",
            "function box(mixed $v): mixed {return $v;} echo box([1]) - 1;",
            "Unsupported operand types",
        ),
        (
            "zero",
            "function box(mixed $v): mixed {return $v;} echo box(1) / 0;",
            "DivisionByZeroError",
        ),
        (
            "shift",
            "function box(mixed $v): mixed {return $v;} echo box(1) << -1;",
            "ArithmeticError",
        ),
    ] {
        let file = temp_php(&format!("mixed_runtime_{name}"), source);
        let r = run_ok(&["run", file.to_str().unwrap()]);
        assert_eq!(r.code, 255, "{name}: {}", r.stderr);
        assert!(r.stderr.contains(message), "{name}: {}", r.stderr);
    }
}

fn assert_file_matches_php(file: &std::path::Path) {
    if !have_cc() || !have_php() {
        return;
    }
    let native = run_ok(&["run", file.to_str().unwrap()]);
    let php = Command::new("php").arg(file).output().unwrap();
    assert!(
        php.status.success(),
        "{}",
        String::from_utf8_lossy(&php.stderr)
    );
    assert_eq!(native.code, 0, "{}", native.stderr);
    assert_eq!(native.stdout.as_bytes(), php.stdout);
}

#[test]
fn gradual_migration_project_matches_php() {
    let entry = temp_php(
        "migration_project",
        include_str!("../../../examples/migration/main.php"),
    );
    std::fs::write(
        entry.parent().unwrap().join("legacy.php"),
        include_str!("../../../examples/migration/legacy.php"),
    )
    .unwrap();
    std::fs::write(
        entry.parent().unwrap().join("catalog.csv"),
        include_str!("../../../examples/migration/catalog.csv"),
    )
    .unwrap();
    assert_file_matches_php(&entry);
}

#[test]
fn php_interop_preserves_state_output_binary_data_and_include_modes() {
    let entry = temp_php(
        "interop_order",
        r#"<?php
require __DIR__ . '/legacy.php';
echo 'native|';
include_once __DIR__ . '/legacy.php';
include __DIR__ . '/legacy.php';
require_once __DIR__ . '/legacy.php';
echo legacyCount() . "\n";
$value = legacyEcho("A\x00\xff\n");
if (is_string($value)) { echo bin2hex($value) . "\n"; }
$record = legacyEcho(['text' => 'native', 'count' => 2, 'nested' => [false, null, 1.5], '01' => -4]);
if (is_array($record)) { echo $record['text'] . ':' . $record['count'] . ':' . $record['01'] . "\n"; }
echo legacyEcho(9223372036854775807) . "\n";
echo legacyEcho(-9223372036854775807 - 1) . "\n";
$big = legacyEcho(str_repeat('x', 200000));
if (is_string($big)) { echo strlen($big) . "\n"; }
echo legacyCount() + 1;
"#,
    );
    std::fs::write(
        entry.parent().unwrap().join("legacy.php"),
        r#"<?php
// @pxr-dynamic
$GLOBALS['loads'] = ($GLOBALS['loads'] ?? 0) + 1;
echo 'php|';
if (!function_exists('legacyCount')) {
    function legacyCount() { return $GLOBALS['loads']; }
    function legacyEcho($value) { echo 'call|'; return $value; }
}
"#,
    )
    .unwrap();
    assert_file_matches_php(&entry);
}

#[test]
fn mixed_native_and_php_object_stringification_match_php() {
    let entry = temp_php(
        "interop_objects",
        r#"<?php
require_once __DIR__ . '/legacy.php';
class Label { public function __toString(): string { echo 'native string|'; return 'native'; } }
class ChildLabel extends Label { public function __toString(): string { return 'child'; } }
function boxObject(mixed $value): mixed { return $value; }
$value = boxObject(new Label());
echo $value . ':' . strval(boxObject(new ChildLabel())) . ':' . strval(new ChildLabel()) . "\n";
echo ($value == 'native') . "\n";
$proxy = legacyLabel();
echo $proxy . ':' . ($proxy === legacyLabel()) . "\n";
echo ($proxy == 'php') . ':' . ($proxy <=> 'php') . ':' . ($proxy != 'other') . "\n";
"#,
    );
    std::fs::write(
        entry.parent().unwrap().join("legacy.php"),
        r#"<?php
// @pxr-dynamic
class LegacyLabel { public function __toString() { echo 'php string|'; return 'php'; } }
function legacyLabel() { static $label; return $label ??= new LegacyLabel(); }
"#,
    )
    .unwrap();
    assert_file_matches_php(&entry);
}

#[test]
fn php_interop_resolves_namespaced_calls_and_global_fallback() {
    let entry = temp_php(
        "interop_namespaces",
        r#"<?php
namespace App;
require_once __DIR__ . '/legacy.php';
echo namespacedValue() . ':' . globalValue() . ':' . \globalValue() . "\n";
echo is_int('text') . ':' . \is_int('text') . "\n";
"#,
    );
    std::fs::write(
        entry.parent().unwrap().join("legacy.php"),
        r#"<?php
// @pxr-dynamic
namespace App { function namespacedValue() { return 'namespace'; } function is_int($v) { return true; } }
namespace { function globalValue() { return 'global'; } }
"#,
    )
    .unwrap();
    assert_file_matches_php(&entry);
}

#[test]
fn php_interop_reports_errors_and_preserves_exit_status() {
    if !have_cc() || !have_php() {
        return;
    }
    for (name, legacy, call, code, message) in [
        (
            "exception",
            "function legacyFailure() { throw new RuntimeException('legacy failed'); }",
            "legacyFailure();",
            255,
            "legacy failed",
        ),
        (
            "reference",
            "function legacyFailure(&$v) { $v++; }",
            "legacyFailure(1);",
            255,
            "by-reference arguments",
        ),
        (
            "resource",
            "function legacyFailure() { return fopen('php://memory', 'r'); }",
            "legacyFailure();",
            255,
            "resources cannot cross",
        ),
        (
            "unknown",
            "",
            "missingLegacyFunction();",
            255,
            "does not exist",
        ),
        ("exit", "exit(42);", "", 42, ""),
        ("exit_zero", "exit(0);", "", 0, ""),
        (
            "native_object",
            "function legacyFailure($v) { return $v; }",
            "class C {} legacyFailure(new C());",
            255,
            "native objects cannot cross",
        ),
    ] {
        let entry = temp_php(
            &format!("interop_failure_{name}"),
            &format!("<?php\nrequire __DIR__ . '/legacy.php';\n{call}"),
        );
        std::fs::write(
            entry.parent().unwrap().join("legacy.php"),
            format!("<?php\n// @pxr-dynamic\n{legacy}"),
        )
        .unwrap();
        let result = run_ok(&["run", entry.to_str().unwrap()]);
        assert_eq!(result.code, code, "{name}: {}", result.stderr);
        assert!(result.stderr.contains(message), "{name}: {}", result.stderr);
    }
}

#[test]
fn migration_report_describes_native_and_dynamic_boundaries_without_execution() {
    let entry = temp_php(
        "migration_report_\"é",
        r#"<?php
require_once __DIR__ . '/legacy.php';
function typed(int $value): int { return $value + 1; }
function gradual(mixed $value): mixed { return $value; }
echo legacyName();
"#,
    );
    let marker = entry.parent().unwrap().join("executed.txt");
    let legacy = format!("<?php\n// @pxr-dynamic\nfile_put_contents(__DIR__ . '/executed.txt', 'executed');\nfunction legacyName() {{ return 'legacy'; }}");
    std::fs::write(entry.parent().unwrap().join("legacy.php"), legacy).unwrap();
    let report = run_ok(&["migration-report", entry.to_str().unwrap(), "--json"]);
    assert_eq!(report.code, 0, "{}", report.stderr);
    assert!(report.stdout.contains("\"checked\":true"));
    assert!(report.stdout.contains("\"mode\":\"php\""));
    assert!(report.stdout.contains("\"mode\":\"native\""));
    assert!(report.stdout.contains("\"name\":\"typed\""));
    assert!(report.stdout.contains("\"mixed_return\":true"));
    assert!(report.stdout.contains("\"dynamic_calls\":1"));
    assert!(!marker.exists(), "report executed the legacy file");
    if have_php() {
        let json = entry.parent().unwrap().join("report.json");
        std::fs::write(&json, &report.stdout).unwrap();
        let parsed = Command::new("php")
            .args([
                "-r",
                "json_decode(file_get_contents($argv[1]), true, 512, JSON_THROW_ON_ERROR);",
            ])
            .arg(json)
            .output()
            .unwrap();
        assert!(
            parsed.status.success(),
            "{}",
            String::from_utf8_lossy(&parsed.stderr)
        );
    }
    let invalid = temp_php("migration_invalid", "function untyped($x) { return $x; }");
    let failed = run_ok(&["migration-report", invalid.to_str().unwrap(), "--json"]);
    assert_ne!(failed.code, 0);
    assert!(failed.stdout.contains("\"checked\":false"));
    assert!(failed.stderr.contains("E0202"));
}

#[test]
fn php_is_required_only_when_the_interop_boundary_executes() {
    if !have_cc() {
        return;
    }
    let entry = temp_php("no_php_dependency", "echo \"native\\n\";");
    let executable = entry.with_extension("bin");
    let built = run_ok(&[
        "build",
        entry.to_str().unwrap(),
        "-o",
        executable.to_str().unwrap(),
    ]);
    assert_eq!(built.code, 0, "{}", built.stderr);
    let native = Command::new(&executable)
        .env("PXR_PHP", "/missing/php-executable")
        .output()
        .unwrap();
    assert!(native.status.success());
    assert_eq!(native.stdout, b"native\n");
    let blocked_cache = entry.parent().unwrap().join("not-a-cache-directory");
    std::fs::write(&blocked_cache, "file").unwrap();
    let uncached = Command::new(pxr())
        .args([
            "build",
            entry.to_str().unwrap(),
            "-o",
            executable.to_str().unwrap(),
        ])
        .env("PXR_CACHE_DIR", blocked_cache)
        .output()
        .unwrap();
    assert!(
        uncached.status.success(),
        "{}",
        String::from_utf8_lossy(&uncached.stderr)
    );
    let dynamic = temp_php("missing_php_dependency", "require __DIR__ . '/legacy.php';");
    std::fs::write(
        dynamic.parent().unwrap().join("legacy.php"),
        "<?php\n// @pxr-dynamic\necho 'legacy';",
    )
    .unwrap();
    let executable = dynamic.with_extension("bin");
    let built = run_ok(&[
        "build",
        dynamic.to_str().unwrap(),
        "-o",
        executable.to_str().unwrap(),
    ]);
    assert_eq!(built.code, 0, "{}", built.stderr);
    let legacy = Command::new(&executable)
        .env("PXR_PHP", "/missing/php-executable")
        .output()
        .unwrap();
    assert_eq!(legacy.status.code(), Some(127));
    assert!(String::from_utf8_lossy(&legacy.stderr).contains("PHP interop needs a PHP CLI"));
}

#[test]
fn conditional_php_includes_are_lazy_and_preserve_once_semantics() {
    let entry = temp_php(
        "conditional_interop",
        r#"<?php
if (false) { require_once __DIR__ . '/legacy.php'; }
function loadLegacy(): void { include_once __DIR__ . '/legacy.php'; }
loadLegacy(); loadLegacy();
require_once __DIR__ . '/legacy.php';
echo legacyConditional() . "\n";
"#,
    );
    std::fs::write(
        entry.parent().unwrap().join("legacy.php"),
        "<?php\n// @pxr-dynamic\necho 'loaded|'; function legacyConditional() { return 'yes'; }",
    )
    .unwrap();
    assert_file_matches_php(&entry);
    if have_cc() {
        let lazy = temp_php(
            "unexecuted_interop",
            "if (false) { require_once __DIR__ . '/legacy.php'; } echo 'native';",
        );
        std::fs::write(
            lazy.parent().unwrap().join("legacy.php"),
            "<?php\n// @pxr-dynamic\necho 'wrong';",
        )
        .unwrap();
        let executable = lazy.with_extension("bin");
        let built = run_ok(&[
            "build",
            lazy.to_str().unwrap(),
            "-o",
            executable.to_str().unwrap(),
        ]);
        assert_eq!(built.code, 0, "{}", built.stderr);
        let result = Command::new(executable)
            .env("PXR_PHP", "/missing/php")
            .output()
            .unwrap();
        assert!(result.status.success());
        assert_eq!(result.stdout, b"native");
    }
}

#[test]
fn php_float_special_values_and_nested_object_comparisons_match_php() {
    let entry = temp_php(
        "interop_special_values",
        r#"<?php
require __DIR__ . '/legacy.php';
function boxSpecial(mixed $v): mixed { return $v; }
$nan = legacyNan();
$values = [boxSpecial('NAN'), boxSpecial('a'), boxSpecial(0), boxSpecial(false), boxSpecial(true), boxSpecial(null), boxSpecial([]), $nan];
foreach ($values as $v) { echo ($nan == $v) . ':' . ($nan < $v) . ':' . ($nan > $v) . ':' . ($nan <=> $v) . '|'; }
echo "\n";
echo -boxSpecial(0.0) . ':' . -boxSpecial(-0.0) . "\n";
$nativeArray = [$nan]; $nativeCopy = $nativeArray;
echo ($nativeArray === $nativeCopy) . ':' . ($nativeArray === array_values($nativeArray)) . ':' . ($nativeArray === array_merge([], $nativeArray)) . ':' . ($nativeArray === array_merge($nativeArray)) . "\n";
$nativeCopy[0] = $nan; echo ($nativeArray === $nativeCopy) . "\n";
$returned = legacyNanArray(); $alias = $returned;
echo ($returned === $alias) . ':' . ($returned === legacyNanArray()) . ':' . ($returned === legacyEchoArray($returned)) . "\n";
if (is_array($alias)) { $alias[0] = $nan; }
echo ($returned === $alias) . ':' . ($returned === ($returned + [])) . "\n";
$left = boxSpecial([legacyText()]); $right = boxSpecial(['text']);
echo ($left == $right) . "\n";
"#,
    );
    std::fs::write(
        entry.parent().unwrap().join("legacy.php"),
        r#"<?php
// @pxr-dynamic
function legacyNan() { return NAN; }
function legacyNanArray() { static $value = [NAN]; return $value; }
function legacyEchoArray($value) { return $value; }
class LegacyText { public function __toString() { echo 'cast|'; return 'text'; } }
function legacyText() { return new LegacyText(); }
"#,
    )
    .unwrap();
    assert_file_matches_php(&entry);
}

#[test]
fn indexed_arrays_preserve_order_copies_and_append_keys() {
    assert_matches_php(
        "array_index_transitions",
        r#"<?php
$a = []; $i = 0;
while ($i < 128) {$a[] = $i; $i++;}
$b = $a; $b["label"] = 9; $b[-3] = 10; $b["300"] = 11; $b[] = 42;
echo count($a), ":", count($b), ":", $b[301], ":", $a[127], "\n";
$c = [-5 => 11, "note" => 22]; $c[] = 33;
echo $c[-4], ":", array_key_exists(0, $c), ":";
foreach ($c as $v) {echo $v, ":";}
echo "\n";
$d = array_reverse($b, true); $d[] = 99;
echo $d[302], ":", count($d), ":", $b[301], "\n";
"#,
    );
}

#[test]
fn indexed_arrays_normalize_keys_without_merging_distinct_strings() {
    assert_matches_php(
        "array_index_keys",
        r#"<?php
$a = ["0" => 1, "-0" => 2, "01" => 3, "-01" => 4, "+1" => 5,
      "9223372036854775807" => 6, "9223372036854775808" => 7,
      "a\0b" => 8, "a\0c" => 9];
$a[0] = 10; $a["a\0b"] = 11;
echo count($a), ":", $a["-0"], ":", $a["9223372036854775807"], ":", $a["9223372036854775808"], "\n";
foreach ($a as $v) {echo $v, ":";}
echo "\n";
$b = $a; $b["-0"] = 99;
echo $a["-0"], ":", $b["-0"], ":", array_key_exists("missing", $b), "\n";
"#,
    );
}

#[test]
fn appending_after_the_largest_integer_key_fails_without_overflow() {
    if !have_cc() {
        return;
    }
    let file = temp_php(
        "array_append_overflow",
        "<?php $a = [9223372036854775807 => 1]; $a[] = 2; echo \"bad\";",
    );
    let result = run_ok(&["run", file.to_str().unwrap()]);
    assert_eq!(result.code, 255);
    assert!(result.stdout.is_empty());
    assert!(
        result.stderr.contains("next element is already occupied"),
        "{}",
        result.stderr
    );
}
