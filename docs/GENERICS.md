# Generics

M5.1 adds native generic functions; M5.2 adds generic classes and interfaces;
M5.3 adds independent generic methods, M5.4 adds bounds/traits, and M5.5 adds
whole-function return inference. M5 is complete.
Files keep the `.php` extension, but generic
syntax is a PXR extension: these files must be compiled with `pxr`, and do not
parse in the PHP interpreter.

```php
<?php
function identity<T>(T $value): T { return $value; }
function first<T>(array<T> $values, T $fallback): T {
    if (count($values) == 0) { return $fallback; }
    return $values[0];
}

echo identity<int>(42);       // explicit specialization
echo identity('hello');       // inferred identity<string>
echo first([1, 2], 0);        // inferred first<int>
echo first<string>([], '');   // explicit element type for an empty array
```

Type parameters are case-sensitive identifiers scoped to their function. They
can appear in parameter types, an optional return annotation, nested `array<T>`
types, local annotations (`$copy: T = $value;`), and type arguments to other
generic functions. Several parameters are supported, such as
`function second<A, B>(A $a, B $b): B { return $b; }`. Calls follow ordinary
namespace and `use function` lookup, including compile-time declaration modules.

## Inference and checking

Calls may supply all type arguments or omit all of them. Inference uses argument
types and recursively matches array element types. Repeated occurrences of a
parameter must infer exactly the same type: a call with `T` arguments `1` and
`2.5` fails. Supply `<float>` explicitly to allow ordinary scalar `int` to
`float` widening, or `<mixed>` to opt into mixed boxing. Class/interface type
arguments use the existing nominal assignability rules.

Inference does not use the expected return type and does not invent a common
supertype. Empty arrays provide no evidence for an element parameter:
`first([], 0)` infers `int` from the fallback, while a function taking only
`array<T>` needs an explicit type argument for `[]`. A parameter used only in a
return annotation likewise needs an explicit argument. `identity([])` can infer
the whole empty-array type without choosing an element type.

Explicit `mixed` and `array<mixed>` destinations contextualize heterogeneous
array literals. Existing mixed guard and conversion rules still apply inside
the function. Scalar widening does not convert arrays: `array<int>` cannot be
passed as `array<float>`. Deep array element reboxing is likewise not implicit;
the compiler rejects incompatible layouts rather than interpreting their bits
as another type.

`void` cannot be a type argument, though a generic function may return `void`.
Invalid value/type argument counts, unresolved types, conflicting inference,
and calls with explicit type arguments to ordinary functions fail at compile
time. Body errors identify the concrete specialization.

## Native specialization

The analyzer substitutes concrete types and checks each instantiated body before
IR generation. Each distinct function/type tuple produces one native function,
shared by repeated calls. Integer specializations use the integer ABI; there is
no generic dispatch or mandatory mixed boxing. Arrays retain value semantics
and objects retain shared identity through the existing call lowering.

Direct and mutual recursion reuse registered specializations. To bound growing
polymorphic recursion, compilation allows at most 64 specializations per generic
function and 256 in one program; exceeding either emits `E0283`.

Generic function signatures are checked even when unused. Bodies are checked when
instantiated, and unused templates emit no native function. This is a template
model: operations must be valid for each concrete instantiation. Declared bounds
are checked before accepting an instantiation.

## Generic classes and interfaces

```php
class Box<T> {
    public T $value;
    public function __construct(T $value) { $this->value = $value; }
    public function get(): T { return $this->value; }
}
function wrap<T>(T $value): Box<T> { return new Box<T>($value); }
function unwrap<T>(Box<T> $box): T { return $box->get(); }

$number: Box<int> = new Box<int>(42);
$word = wrap('hello');       // Box<string>
echo unwrap($number);       // infers T = int from Box<int>
```

Class type arguments are explicit at construction and in annotations; `new
Box(42)` is an error. Type parameters scope over fields, constructors, method
signatures/bodies, and inheritance headers. Multiple parameters, nested generic
types (`Box<Box<int>>`), recursive fields, array and mixed arguments, namespace
aliases, and declaration modules are supported.

Generic inheritance and interface contracts use ordinary nominal checking:

```php
interface Reader<T> { public function get(): T; }
class ReadableBox<T> extends Box<T> implements Reader<T> {}
class IntBox extends ReadableBox<int> {}
```

Function inference can extract arguments from instantiated generic classes or
their generic ancestors/interfaces. Repeated evidence must agree; conflicting
inherited instantiations do not select an arbitrary type. Distinct instantiations
are invariant: `Box<int>` does not become `Box<float>`, `Box<mixed>`, or
`Box<Parent>` through element coercion. An explicitly declared inheritance edge
still permits the ordinary upcast.

Each instantiation gets its own concrete fields, methods, class ID, and static
storage. `Box<int>::method(...)`, `Box<int>::$property`, `self::`, `parent::`,
virtual dispatch, visibility checks, and `instanceof Box<int>` use that concrete
class. Objects retain shared identity; arrays copied into/out of fields retain
value semantics. Native `__toString`, including direct typed object `echo` and
`print`, uses runtime dispatch.

Class specialization is demand-driven and cached. A generic function can request
a new class through its inferred bindings; the analyzer completes the expanded
class graph before accepting the program. Non-generic members of an instantiated
class are checked even when never called. Generic method signatures are checked
with the class; their bodies are checked when specialized. Uninstantiated class
templates are parsed but not semantically checked or emitted. Duplicate class
names are rejected even without instantiation. Class specialization has its own
limits of 64 instantiations per template, 256 per program, and 16,384 bytes per
expanded type name (`E0283`). The size limit also bounds exponentially growing
type expressions. Finite self-references reuse the cached instance.

## Independent generic methods

Methods can declare parameters independently of their class:

```php
class Factory<T> {
    public function boxed<U>(U $value): Box<U> { return new Box<U>($value); }
    public static function first<U>(array<U> $values): U { return $values[0]; }
}
$factory = new Factory<int>();
$text = $factory->boxed<string>('hello');  // explicit method argument
$number = $factory->boxed(42);             // inferred U = int
echo Factory<int>::first<string>(['ok']);  // class and method arguments differ
```

`$this->method<U>(...)`, `self::method<U>(...)`, and `parent::method<U>(...)`
are supported. Normal receiver/argument evaluation order, visibility, static
access rules, array value semantics, object identity, and recursive calls apply.
Function-style inference can extract method arguments through `array<U>` and
generic class/interface patterns. Namespace aliases and native declaration
modules work as for ordinary methods.

Interfaces and abstract classes can declare generic methods. Overrides must
preserve the number and positions of type parameters and the signature shape;
parameter names can differ (`<U>` and `<V>`). Written return annotations preserve
the contract's shape; omitted implementation returns are checked after concrete
inference against the interface/base result.
Calls through base classes/interfaces specialize potential implementations and
use the existing concrete virtual dispatch. Newly discovered classes acquire
previously requested specializations before the program is accepted. Qualified
parent calls select the parent implementation directly.

Unused method bodies emit no code and are not checked. Repeated calls reuse
their specialization. Body diagnostics identify `Class::method<types>`. Method
expansion permits 64 instantiations per declared method, 256 generated method
declarations per program, and 16,384 bytes per displayed specialization
(`E0283`); virtual/interface calls can materialize multiple implementations.
The generic call-checking budget also applies.

Directly declared method parameters cannot shadow class parameters; composed
trait methods retain their original lexical scope. Constructors and magic
methods cannot declare independent type parameters (`E0282`), though their
signatures may use class parameters as before.

## Bounds and trait contracts

```php
interface Printable { public function text(): string; }
interface Named { public function name(): string; }
function describe<T: Printable & Named>(T $value) {
    return $value->name() . ':' . $value->text();
}
class Holder<T: Printable> { /* typed members using T */ }
```

Bounds apply to functions, classes, and independent methods. A type argument
must satisfy every class/interface bound by nominal ancestry. Method-name
coincidence is not a substitute for implementing an interface. Scalar, array,
and mixed arguments do not satisfy object bounds without a proven narrowing.
Explicit and inferred arguments receive the same checks (`E0286`).

Bounds can contain generic interfaces and other type parameters, including
forward references: `<U: Reader<T>, T>` or `<A, B: A>`. Such dependent bounds are
resolved with the complete concrete binding set. Bounds do not themselves infer
missing arguments: a parameter used only inside a bound may need an explicit
type-argument list. Primitive bounds such as `<T: int>` are rejected.

Overrides and interface implementations preserve their generic bound sets up
to parameter renaming and intersection order; changing those contracts fails
with `E0282`. Native code-sharing traits are documented in [`TRAITS.md`](TRAITS.md).
Interfaces are the capability/trait contracts for bounds; mixin trait names are
not value types or bounds. See [`../examples/constraints.php`](../examples/constraints.php).

## Inferred returns and further extensions

Function and concrete method return annotations may be omitted, including in
generic templates. Results are inferred per specialization, through recursive
calls and class construction. Abstract/interface contracts still require an
annotation. Ambiguous or ungrounded cycles fail with `E0284`; see
[`INFERENCE.md`](INFERENCE.md) for joining rules and limits.

Partial type-argument lists, default type arguments, and constructor type-argument inference
remain unsupported. They produce diagnostics. `owned<T>` and call-scoped borrows
are delivered in M6; see [`OWNERSHIP.md`](OWNERSHIP.md). Async remains planned for M7.

Run [`../examples/generics.php`](../examples/generics.php) with
`pxr run examples/generics.php` for recursion, forwarding, nested arrays,
explicit/inferred calls, and mixed specialization.
[`../examples/generic_classes.php`](../examples/generic_classes.php) demonstrates
concrete boxes, function/class inference, nesting, arrays, and object identity.
[`../examples/generic_methods.php`](../examples/generic_methods.php) demonstrates
independent method parameters, static calls, parent calls, and interface dispatch.
