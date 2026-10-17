# Native callable support

M10 is in progress. Invokable objects, by-value closures and arrow functions
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
methods preserve the consuming class's scope. In non-generic classes, parameter
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
access in non-generic classes. A closure created in a static method also has no
`$this`. Creating a synchronous closure after an await in an async method is
supported; the returned environment outlives the creating frame. Owned parameters/returns and
read/exclusive borrow parameters keep their normal contracts through invocation;
forwarding moves owners and reborrows borrowed parameters. Inferred `void` is
forwarded as a call followed by a bare return.

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
- Closures in generic class/method scope and annotations using enclosing generic
  types remain rejected until generic lexical type binding is supplied.
  Static closures and lexical `self`/`parent` annotations in trait scope also
  remain rejected. `parent` requires a declared parent class.
- Async closures, callable strings/arrays, first-class callable references,
  `callable`/`Closure` annotations and Closure reflection/binding APIs are not
  implemented. Late static binding (`static::`) retains its existing restriction.
- Native callbacks are not implicitly exported across the legacy PHP boundary;
  the existing [interop restrictions](INTEROP.md) still apply.
- Invokable magic methods remain synchronous and cannot declare method generics.
- Untyped parameters, optional/variadic/named arguments and unpacking retain their
  current restrictions; accepting a closure does not bypass argument checks.

Behavioral references: [PHP anonymous functions](https://www.php.net/manual/en/functions.anonymous.php)
and [PHP arrow functions](https://www.php.net/manual/en/functions.arrow.php).
