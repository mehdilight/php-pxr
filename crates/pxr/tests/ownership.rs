//! M6 acceptance: source ownership contracts and precise native reclamation.
use std::path::PathBuf;
use std::process::Command;

fn have_cc() -> bool {
    Command::new(std::env::var("CC").unwrap_or_else(|_| "clang".into()))
        .arg("--version")
        .output()
        .is_ok_and(|o| o.status.success())
}
fn build(name: &str, source: &str, opt: u8) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("pxr-m6-{}-{name}-{opt}", std::process::id()));
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
const OWNERSHIP: &str = r#"<?php
class Counter {
    public int $value;
    public function __construct(int $value) { $this->value = $value; }
    public function add(int $n): void { $this->value += $n; }
    public function transfer<T>(owned<T> $v): owned<T> { return move($v); }
}
function read(&Counter $c): int { return $c->value; }
function increment(&mut Counter $c): void { $c->value += 1; }
function forward<T>(owned<T> $v): owned<T> { return move($v); }
function append(&mut array<int> $a): void { $a[] = 3; $a[0] += 4; }
function pair(&Counter $a, &Counter $b): int { return $a->value + $b->value; }
function reborrow(&mut Counter $c): int { increment(&mut $c); return read(&$c); }
function choose(bool $flag, owned<Counter> $c): int {
    if ($flag) { drop($c); return 1; }
    return read(&$c);
}
function branch(bool $flag): owned<Counter> {
    if ($flag) { return new Counter(8); }
    $c: owned<Counter> = new Counter(9); return move($c);
}
$c: owned<Counter> = new Counter(4);
echo read(&$c), ":";
increment(&mut $c); $c->add(2);
$d = forward(move($c));
echo read(&$d), ":", pair(&$d, &$d), ":", reborrow(&mut $d), "\n";
$a: owned<array<int>> = [1, 2]; append(&mut $a);
echo count($a), ":", $a[0], ":", $a[2], "\n"; drop($a);
$x: owned<string> = "abc";
for ($i = 0; $i < 3; $i++) { $next = forward(move($x)); $x = move($next); }
echo strlen($x), ":";
drop($x); $x = "fresh"; echo strlen($x), "\n";
$e = $d->transfer(branch(true));
echo choose(true, move($e)), ":", choose(false, branch(false)), "\n";
"#;

#[test]
fn owned_values_moves_borrows_and_generic_methods() {
    if !have_cc() {
        return;
    }
    for opt in [0, 3] {
        let binary = build("ownership", OWNERSHIP, opt);
        let out = Command::new(&binary)
            .env("PXR_PHP", "/missing/php")
            .env("PXR_GC_STRESS", "1")
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "opt {opt}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        assert_eq!(out.stdout, b"4:7:14:8\n3:5:3\n3:5\n1:9\n");
        assert!(
            out.stderr.is_empty(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        std::fs::remove_dir_all(binary.parent().unwrap()).unwrap();
    }
}

#[test]
fn ownership_violations_fail_before_lowering() {
    let cases = [
        ("assignment-result-copy", "$x: owned<int> = 1; $y = ($x = 2);"),
        ("increment-result-copy", "$x: owned<int> = 1; echo $x++;"),
        ("copy", "$x: owned<string> = \"x\"; $y = $x;"),
        ("move-use", "$x: owned<string> = \"x\"; $y = move($x); echo $x;"),
        ("drop-use", "$x: owned<string> = \"x\"; drop($x); echo $x;"),
        ("double-move", "$x: owned<int> = 1; $y = move($x); $z = move($x);"),
        ("shared-move", "$x = 1; $y = move($x);"),
        ("branch", "$x: owned<int> = 1; if (true) { drop($x); } echo $x;"),
        ("loop", "$x: owned<int> = 1; while (true) { echo $x; drop($x); }"),
        ("continue", "$x: owned<int> = 1; for ($i=0;$i<2;$i++) { drop($x); continue; }"),
        ("condition", "function take(owned<int> $x): bool { return true; } $x: owned<int> = 1; while (take(move($x))) {}"),
        ("shared-call", "function f(string $x): void {} $x: owned<string> = \"x\"; f($x);"),
        ("implicit-transfer", "function f(owned<int> $x): void {} $x: owned<int> = 1; f($x);"),
        ("implicit-borrow", "function f(&int $x): void {} $x: owned<int> = 1; f($x);"),
        ("shared-alias-borrow", "function f(&int $x): void {} $x = 1; f(&$x);"),
        ("overlap", "function f(&array<int> $a, &mut array<int> $b): void {} $x: owned<array<int>> = [1]; f(&$x, &mut $x);"),
        ("two-exclusive", "function f(&mut array<int> $a, &mut array<int> $b): void {} $x: owned<array<int>> = [1]; f(&mut $x, &mut $x);"),
        ("nested-loan", "function take(owned<array<int>> $x): int { return 1; } function f(&array<int> $a, int $b): void {} $x: owned<array<int>> = [1]; f(&$x, take(move($x)));"),
        ("read-while-exclusive", "function f(&mut array<int> $a, int $b): void {} $x: owned<array<int>> = [1]; f(&mut $x, count($x));"),
        ("upgrade", "function f(&mut array<int> $x): void {} function bad(&array<int> $x): void { f(&mut $x); }"),
        ("borrow-store", "$x: owned<int> = 1; $b = &$x;"),
        ("borrow-echo", "$x: owned<int> = 1; echo &$x;"),
        ("borrow-nested-expression", "$x: owned<string> = \"x\"; echo strlen((&$x) . \"z\");"),
        ("borrow-return", "function f(&array<int> $a): array<int> { return $a; }"),
        ("borrow-result-type", "function f(&int $a): &int { return $a; }"),
        ("borrow-local-type", "$x: &int = 1;"),
        ("borrow-property", "class X { public &int $x; }"),
        ("borrow-mutation", "function f(&array<int> $a): void { $a[] = 1; }"),
        ("borrow-compound", "function f(&array<int> $a): void { $a[0] += 1; }"),
        ("borrow-rebind", "function f(&mut array<int> $a): void { $a = []; }"),
        ("borrow-scalar-mut", "function f(&mut int $a): void { $a++; }"),
        ("owner-in-array", "$x: owned<string> = \"x\"; $a = [$x];"),
        ("owner-in-property", "class X { public mixed $value = null; } $x: owned<int> = 1; $o = new X; $o->value = $x;"),
        ("nested-owner-type", "$x: array<owned<int>> = [];"),
        ("owner-field-type", "class X { public owned<int> $x; }"),
        ("shared-initializer", "class X {} $a = new X; $x: owned<X> = $a;"),
        ("escaping-constructor", "class X { public static mixed $leaked=null; public function __construct() { X::$leaked = $this; } } $x: owned<X> = new X;"),
        ("escaping-method", "class X { public function alias(): X { return $this; } } $x: owned<X> = new X;"),
        ("shared-method", "class X { public function f(): void {} } function bad(&X $x): void { $x->f(); }"),
        ("owner-return-shared", "function f(owned<int> $a): int { return $a; }"),
        ("owned-return-copy", "function f(owned<int> $a): owned<int> { return $a; }"),
        ("changed-mode", "class A { public function f(owned<int> $x): void {} } class B extends A { public function f(int $x): void {} }"),
        ("inherited-mode", "interface I { public function f(owned<int> $x): void; } class A { public function f(int $x): void {} } class B extends A implements I {}"),
        ("mode-join", "if (true) { $x = 1; } else { $x: owned<int> = 2; } echo $x;"),
        ("foreach-rebind", "$x: owned<int> = 1; foreach ([2] as $x) {}"),
        ("drop-expression", "$x: owned<int> = 1; $y = drop($x);"),
        ("drop-types", "$x: owned<int> = 1; drop<int>($x);"),
        ("move-target", "$x: owned<array<int>> = [1]; $y = move($x[0]);"),
        ("do-condition-repeat", "function take(owned<int> $x): bool { return true; } $x: owned<int> = 1; do {} while (take(move($x)));"),
        ("for-update-repeat", "function take(owned<int> $x): void {} $x: owned<int> = 1; for ($i=0;$i<2;take(move($x))) {}"),
        ("switch-case-transfer", "function take(owned<int> $x): int { return 1; } $x: owned<int> = 1; switch(0) { case take(move($x)): break; case take(move($x)): break; }"),
        ("match-case-transfer", "function take(owned<int> $x): int { return 1; } $x: owned<int> = 1; echo match(0) { take(move($x)) => 1, take(move($x)) => 2, default => 3 };"),
        ("borrow-rebox", "function f(&mut array<mixed> $a): void { $a[]=true; } $a: owned<array<int>>=[1]; f(&mut $a);"),
        ("borrow-covariance", "class A {} class B extends A {} function f(&mut array<A> $a): void { $a[]=new A; } $a: owned<array<B>>=[new B]; f(&mut $a);"),
        ("static-factory-leak", "class X { public static mixed $leaked=null; public function __construct() { X::$leaked=$this; } } class Factory { public static function create(): owned<X> { return new X; } }"),
        ("conditional-constructor-leak", "class X { public static mixed $leaked=null; public function __construct() { X::$leaked=$this; } } $x: owned<X> = true ? new X : new X;"),
        ("borrow-explicit-rebox", "function f<T>(&mut array<T> $a): void {} $a: owned<array<int>>=[1]; f<mixed>(&mut $a);"),
        ("borrow-outer-generic-rebox", "function f<T>(&mut array<T> $a): void {} function g<U>(owned<array<int>> $a): void { f<U>(&mut $a); } g<mixed>([1]);"),
        ("borrow-constructor-rebox", "class C<T> { public function __construct(&mut array<T> $a) { $a[]=null; } } $a: owned<array<int>>=[1]; $c = new C<mixed>(&mut $a);"),
        ("borrow-static-rebox", "class C<T> { public static function f(&mut array<T> $a): void {} } $a: owned<array<int>>=[1]; C<mixed>::f(&mut $a);"),
        ("borrow-parent-rebox", "class B<T> { public function f(&mut array<T> $a): void {} } class C<U> extends B<mixed> { public function g(owned<array<int>> $a): void { parent::f(&mut $a); } }"),
        ("borrow-to-shared-scalar", "function f(int $x): void {} $x: owned<int>=1; f(&$x);"),
        ("shared-implicit-magic", "class X { public function __toString(): string { return \"x\"; } } function f(&X $x): void { echo $x; }"),
        ("shared-strval-magic", "class X { public function __toString(): string { return \"x\"; } } function f(&X $x): string { return strval($x); }"),
        ("shared-comparison-magic", "class X {} function f(&X $x): bool { return $x == \"x\"; }"),
        ("shared-switch-magic", "class X {} function f(&X $x): void { switch($x) { case \"x\": break; } }"),
        ("shared-loan-implicit-magic", "class X { public function __toString(): string { return \"x\"; } } function f(&X $x, string $s): void {} $x: owned<X>=new X; f(&$x, strval($x));"),
        ("native-destructor", "class X { public function __destruct(): void { echo \"bye\"; } } $x = new X;"),
        ("void-owner", "function f(): owned<void> {}"),
        ("reserved-intrinsic", "function move(int $x): int { return $x; }"),
    ];
    for (name, body) in cases {
        let (program, diagnostics) = pxr_driver::check_source(&format!("<?php {body}"));
        assert!(program.is_none(), "{name}: unexpectedly accepted");
        assert!(
            diagnostics.items().iter().any(|d| d.code == Some("E0290")),
            "{name}: {:?}",
            diagnostics.items()
        );
    }
}

#[test]
fn shared_values_and_cycles_have_bounded_memory() {
    if !have_cc() {
        return;
    }
    let source = r#"<?php
class Node { public string $text = ""; public mixed $next = null; public static mixed $keep = null; }
function garbage(): void {
    $a = new Node; $b = new Node;
    $a->text = str_repeat("x", 4096); $a->next = $b; $b->next = $a;
    $record: array<mixed> = [$a, ["binary\0key" => $a->text], 123];
}
function early(bool $flag): string {
    $array = [str_repeat("r", 4096)];
    if ($flag) { return $array[0]; }
    return "fallback";
}
$result = "";
$keep = new Node; $keep->text = "persistent"; $keep->next = $keep; Node::$keep = $keep;
for ($i = 0; $i < 10000; $i++) {
    garbage(); $result = early(true);
    if ($i % 2 == 0) { continue; }
    if ($i == 9999) { break; }
}
$last = Node::$keep;
if ($last instanceof Node) { echo $last->text, ":"; }
echo strlen($result), "\n";
"#;
    for opt in [0, 3] {
        let binary = build("bounded", source, opt);
        let out = Command::new(&binary)
            .env("PXR_PHP", "/missing/php")
            .env("PXR_GC_STATS", "1")
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        assert_eq!(out.stdout, b"persistent:4096\n");
        let stderr = String::from_utf8(out.stderr).unwrap();
        let stats = stderr.trim().strip_prefix("pxr-gc: ").unwrap();
        let entries: std::collections::HashMap<_, _> = stats
            .split_whitespace()
            .map(|s| s.split_once('=').unwrap())
            .collect();
        assert!(
            entries["collections"].parse::<usize>().unwrap() > 10,
            "{stats}"
        );
        assert!(
            entries["peak_bytes"].parse::<usize>().unwrap() < 4 * 1024 * 1024,
            "{stats}"
        );
        std::fs::remove_dir_all(binary.parent().unwrap()).unwrap();
    }
}

#[test]
fn ownership_modules_traits_and_virtual_contracts() {
    if !have_cc() {
        return;
    }
    let source = r#"<?php
namespace Ownership;
interface Consumer { public function take<T>(owned<T> $v): owned<T>; }
trait Transfer { public function take<T>(owned<T> $v): owned<T> { return move($v); } }
class Service implements Consumer { use Transfer; }
function route(Consumer $service, owned<string> $v): owned<string> { return $service->take(move($v)); }
function inspect(&string $v): string { return $v; }
$service = new Service;
$v: owned<string> = "module";
$out = route($service, move($v)); echo inspect(&$out), "\n";
"#;
    let binary = build("contracts", source, 3);
    let out = Command::new(&binary)
        .env("PXR_GC_STRESS", "1")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(out.stdout, b"module\n");
    assert!(out.stderr.is_empty());
    std::fs::remove_dir_all(binary.parent().unwrap()).unwrap();
}

#[test]
fn php_transfer_handles_follow_native_liveness() {
    if !have_cc()
        || !Command::new("php")
            .arg("--version")
            .output()
            .is_ok_and(|o| o.status.success())
    {
        return;
    }
    let name = "php-handles";
    let dir = std::env::temp_dir().join(format!("pxr-m6-{}-{name}-3", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let legacy = dir.join("legacy.php");
    std::fs::write(&legacy, r#"<?php
// @pxr-dynamic
$GLOBALS['destroyed'] = 0;
class Payload {
    public string $data;
    public function __construct() { $this->data = str_repeat('x', 65536); }
    public function __destruct() { $GLOBALS['destroyed']++; }
}
function retained() { return $GLOBALS['retained'] = new Payload; }
function payload() { return [new Payload, str_repeat('a', 65536)]; }
function identity_retained($value) { return $value === $GLOBALS['retained']; }
function retained_array() { return [NAN, $GLOBALS['retained']]; }
function first_is_nan($value) { return is_nan($value[0]); }
function handle_report() {
    gc_collect_cycles();
    return $GLOBALS['destroyed'] >= 199 && memory_get_usage() < 4 * 1024 * 1024 ? 'released' : 'retained';
}
"#).unwrap();
    let source = format!(
        r#"<?php
require '{}';
$keep = retained();
$array = retained_array();
for ($i = 0; $i < 200; $i++) {{ $garbage = payload(); }}
echo handle_report(), ":", identity_retained($keep), ":", first_is_nan($array), "\n";
"#,
        legacy.display()
    );
    let binary = build(name, &source, 3);
    let out = Command::new(&binary)
        .env("PXR_GC_STRESS", "1")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(
        out.stdout,
        b"released:1:1\n",
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        out.stderr.is_empty(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn runtime_callbacks_and_payloads_survive_stress_collection() {
    if !have_cc() {
        return;
    }
    let source = r#"<?php
class Text {
    public string $value;
    public function __construct(string $value) { $this->value = $value; }
    public function __toString(): string {
        for ($i=0; $i<3000; $i++) { $temporary = str_repeat("x", 4096); }
        return $this->value;
    }
}
function hold(string $value): string { $garbage = [str_repeat("a", 4096)]; return $value; }
$first: mixed = new Text("first"); $second: mixed = new Text("second");
$pair: array<mixed> = [$first, $second];
echo $first, ":", $second, ":", $first == "first", "\n";
$array = [str_repeat("k", 2) => hold("value")];
echo $array["kk"] . hold("!"), "\n";
$object = new Text("field");
echo $object->value . hold("!"), "\n";
if (true) { $branch = [hold("branch")]; } else { $branch = [hold("other")]; }
echo $branch[0], "\n";
"#;
    for opt in [0, 3] {
        let binary = build("callbacks", source, opt);
        for stress in [false, true] {
            let mut command = Command::new(&binary);
            command.env("PXR_GC_STATS", "1");
            if stress {
                command.env("PXR_GC_STRESS", "1");
            }
            let out = command.output().unwrap();
            assert!(
                out.status.success(),
                "{opt}/{stress}: {}",
                String::from_utf8_lossy(&out.stderr)
            );
            assert_eq!(out.stdout, b"first:second:1\nvalue!\nfield!\nbranch\n");
            let stderr = String::from_utf8(out.stderr).unwrap();
            assert!(
                !stderr.contains("Sanitizer") && !stderr.contains("runtime error:"),
                "{stderr}"
            );
            let peak = stderr
                .split("peak_bytes=")
                .nth(1)
                .unwrap()
                .split_whitespace()
                .next()
                .unwrap()
                .parse::<usize>()
                .unwrap();
            assert!(peak < 4 * 1024 * 1024, "{stderr}");
        }
        std::fs::remove_dir_all(binary.parent().unwrap()).unwrap();
    }
}

#[test]
fn explicit_drop_releases_transferred_arguments_before_exit() {
    if !have_cc() {
        return;
    }
    let source = r#"<?php
class Container {
    public string $text;
    public function __construct(string $text) { $this->text = $text; }
}
function release(owned<Container> $value): void { drop($value); }
$value: owned<Container> = new Container(str_repeat("x", 65536));
release(move($value));
echo "released\n";
"#;
    let binary = build("drop", source, 3);
    let out = Command::new(&binary)
        .env("PXR_GC_STATS", "1")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(out.stdout, b"released\n");
    let stderr = String::from_utf8(out.stderr).unwrap();
    assert!(stderr.trim().ends_with("remaining_bytes=0"), "{stderr}");
    std::fs::remove_dir_all(binary.parent().unwrap()).unwrap();
}

#[test]
fn ownership_flows_through_modules_and_generic_class_ancestors() {
    if !have_cc() {
        return;
    }
    let name = "owned-modules";
    let dir = std::env::temp_dir().join(format!("pxr-m6-{}-{name}-3", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let module = dir.join("values.php");
    std::fs::write(
        &module,
        r#"<?php
namespace Memory;
class Box<T> {
    public T $value;
    public function __construct(T $value) { $this->value = $value; }
    public function copyValue(): T { return $this->value; }
}
class Child<U> extends Box<array<U>> {}
class Factory<T> {
    public function __construct(&mut array<T> $values, T $value) { $values[] = $value; }
    public static function append(&mut array<T> $values, T $value): void { $values[] = $value; }
    public static function fresh<U>(U $value): owned<Box<U>> { return new Box<U>($value); }
}
class Base<T> { public function append(&mut array<T> $values, T $value): void { $values[] = $value; } }
class Derived<U> extends Base<U> {
    public function process(owned<array<U>> $values, U $value): owned<array<U>> {
        parent::append(&mut $values, $value); return move($values);
    }
}
function relay<T>(owned<T> $value): owned<T> { return move($value); }
function inspect<T>(&Box<T> $value): T { return $value->value; }
"#,
    )
    .unwrap();
    let source = format!(
        r#"<?php
namespace Application;
use Memory\Box;
use Memory\Child;
use function Memory\relay as forward;
use function Memory\inspect as observe;
require '{}';
$a: owned<Box<int>> = new Box<int>(42);
$b = forward(move($a));
echo observe(&$b), ":", $b->copyValue(), "\n";
$c: owned<Child<string>> = new Child<string>(["ancestor"]);
$values = $c->copyValue(); echo $values[0], "\n";
$numbers: owned<array<int>> = [1];
$factory = new \Memory\Factory<int>(&mut $numbers, 2);
\Memory\Factory<int>::append(&mut $numbers, 3);
$derived = new \Memory\Derived<int>;
$processed = $derived->process(move($numbers), 4);
echo count($processed), ":", $processed[3], "\n";
$made = \Memory\Factory<int>::fresh<string>("factory");
echo $made->copyValue(), "\n";
"#,
        module.display()
    );
    let binary = build(name, &source, 3);
    let out = Command::new(&binary)
        .env("PXR_GC_STRESS", "1")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(out.stdout, b"42:42\nancestor\n4:4\nfactory\n");
    assert!(out.stderr.is_empty());
    std::fs::remove_dir_all(dir).unwrap();
}
