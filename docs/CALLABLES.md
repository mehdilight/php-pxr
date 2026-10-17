# Native callable support

M10 is in progress. Invokable objects, by-value closures and synchronous/async arrow functions
have independent native acceptance tests. This is not yet the full PHP callable
contract.

```php
$offset = 10;
$add = function (int $value) use ($offset): int {
    return $value + $offset;
};
$double = fn(int $value): int => $value * 2;
echo $add(3), ":", $double(4), "\n";
```

Closure parameters follow the native explicit-type rule. Returns may be annotated
or inferred. `use ($value)` snapshots a value at creation. Arrow functions capture
free variables by value automatically. Arrays preserve copy-on-write snapshots;
objects retain their shared identity. Captured locals start from the snapshot on
every invocation, so incrementing a by-value capture does not change later calls
or its original binding. Ordinary locals remain local to each call.

Closures may escape their creating function, nest, and stay alive across an await
in the caller or an argument. Environments use the existing typed object layout,
generic specialization, ownership checks and precise GC. Each closure expression
has a distinct inferred concrete native type. Existing fixed-storage rules still
apply when assigning a different closure expression to the same variable.

Closures in ordinary class methods retain their instance receiver, including
when the body does not read `$this`. Bodies execute with the declaring class's
visibility and `self`/`parent` context. Inherited and nested closures keep that
scope while ordinary virtual calls still dispatch to overrides. Instance trait
methods preserve the consuming class's scope. In ordinary and generic classes, parameter
and return annotations may use `self` and `parent`, including inside native
container and ownership types. `self` names the declaring class even when the
factory is inherited; `parent` names its declared parent.

```php
class Prefix {
    private string $text;
    public function __construct(string $text) { $this->text = $text; }
    public function formatter() {
        return fn(string $value): string => $this->text . $value;
    }
}
$format = (new Prefix("kept:"))->formatter();
echo $format("value"), "\n";
```

`static function (...) { ... }` and `static fn(...) => ...` do not retain an
instance receiver. They retain ordinary class visibility, including `self::`
access in ordinary and generic classes. A closure created in a static method also has no
`$this`. Creating a closure after an await in an async method is
supported; the returned environment outlives the creating frame. Owned parameters/returns and
read/exclusive borrow parameters keep their normal contracts through invocation;
forwarding moves owners and reborrows borrowed parameters. Inferred `void` is
forwarded as a call followed by a bare return.

## Enclosing generic types

Closures in generic functions, classes and methods retain enclosing type
parameters and their bounds. This includes parameters used only in annotations,
without a captured value from which to infer them. Nested and async closures
keep those bindings after the creating frame returns. `self` includes the
declaring class's type arguments; `parent` keeps its declared type arguments.

```php
function identity<T>() {
    return fn(T $value): T => $value;
}
$integer = identity<int>();
echo $integer(42), "\n";
```

Each enclosing specialization creates a concrete closure type. Invocation still
checks concrete argument/return types and ownership modes. The closure itself
does not declare method-level type parameters. This uses PXR generics and does
not execute in PHP; see [generic_closures.php](../examples/generic_closures.php).

## Async closures

PXR also supports `async function` and `async fn`. Combine `static` and `async`
to avoid retaining an instance; either modifier order is accepted.

```php
async function main(): void {
    $offset = 10;
    $add = async function(int $value) use ($offset): int {
        await sleepAsync(0);
        return $value + $offset;
    };
    $task = $add(3); // Arguments are captured now; the body has not run.
    echo await $task, "\n";
    $double = static async fn(int $value): int => $value * 2;
    $running = spawn $double(4);
    echo await $running, "\n";
}
```

The result annotation names the completed value. Calling creates an affine lazy
task, with the same await, spawn, cancellation, scope cleanup and moved-owner
rules as an ordinary [async function](ASYNC.md). Captures snapshot when the
closure is created; arguments snapshot when the task is created. Each invocation
starts its captured locals afresh. Bound closures retain lexical visibility and
shared receivers across suspension, including instance trait scopes. Inferred
and explicit void results are supported. Call-scoped borrow parameters and
owned/borrowed captures remain rejected, since a task can outlive the call.
Async syntax is a PXR extension and does not execute in PHP. Bound/class-scoped
async bodies currently run through an awaited scope bridge, adding one internal
task/frame per invocation compared with an unbound global async closure.

A class may implement `__invoke` and be called with `$object($arg)`. Typed
interfaces declaring `__invoke` support callback parameters and virtual dispatch.
Receiver expressions evaluate once before arguments, including awaited arguments.
See [callback_objects.php](../examples/callback_objects.php) and
[closures.php](../examples/closures.php).

## Explicit limits

- Reference captures are rejected until a persistent alias lifetime contract is
  implemented. Call-scoped borrows cannot substitute for a PHP reference capture.
- Owned and borrowed bindings cannot be captured implicitly into shared closure
  storage. They retain the ordinary ownership diagnostics.
- Implicit receiver capture from an owned or borrowed receiver is rejected,
  even if the closure body does not read `$this`. Static closures can escape an
  owned receiver because they do not capture it.
- Static closures and lexical `self`/`parent` annotations in trait scope
  remain rejected. `parent` requires a declared parent class.
- Callable strings/arrays, first-class callable references,
  `callable`/`Closure` annotations and Closure reflection/binding APIs are not
  implemented. Late static binding (`static::`) retains its existing restriction.
- Native callbacks are not implicitly exported across the legacy PHP boundary;
  the existing [interop restrictions](INTEROP.md) still apply.
- User-declared invokable magic methods remain synchronous and cannot declare
  method generics. Async closures use compiler-generated invocation methods.
- Untyped parameters, optional/variadic/named arguments and unpacking retain their
  current restrictions; accepting a closure does not bypass argument checks.

Behavioral references: [PHP anonymous functions](https://www.php.net/manual/en/functions.anonymous.php)
and [PHP arrow functions](https://www.php.net/manual/en/functions.arrow.php).
