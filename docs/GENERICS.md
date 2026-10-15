# Generics

M5.1 adds native generic functions; M5.2 adds generic classes and interfaces.
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
can appear in parameter types, the required return annotation, nested `array<T>`
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
model: operations must be valid for each concrete instantiation; declared
constraints are not implemented yet.

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
class graph before accepting the program. All members of an instantiated class
are checked, including methods that are never called. Uninstantiated class
templates are parsed but not semantically checked or emitted. Duplicate class
names are rejected even without instantiation. Class specialization has its own
limits of 64 instantiations per template, 256 per program, and 16,384 bytes per
expanded type name (`E0283`). The size limit also bounds exponentially growing
type expressions. Finite self-references reuse the cached instance.

## Remaining M5 work

Methods may use their class's parameters. Methods declaring independent type
parameters, constraints/traits, omitted return annotations, partial type
argument lists, default type arguments, and constructor type-argument inference
remain unsupported. They produce diagnostics. Async remains planned for M7
after the ownership work in M6.

Run [`../examples/generics.php`](../examples/generics.php) with
`pxr run examples/generics.php` for recursion, forwarding, nested arrays,
explicit/inferred calls, and mixed specialization.
[`../examples/generic_classes.php`](../examples/generic_classes.php) demonstrates
concrete boxes, function/class inference, nesting, arrays, and object identity.
