# Native callable support

M10 is in progress. Invokable objects, local by-value closures and arrow functions
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
- Closures declared in class methods, `$this`, lexical class access and closure
  annotations referring to enclosing generic types are rejected until lexical
  receiver/type binding is implemented.
- Static and async closures, callable strings/arrays, first-class callable
  references, `callable`/`Closure` annotations and Closure reflection/binding APIs
  are not implemented. These forms remain compile errors.
- Invokable magic methods remain synchronous and cannot declare method generics.
- Untyped parameters, optional/variadic/named arguments and unpacking retain their
  current restrictions; accepting a closure does not bypass argument checks.

Behavioral references: [PHP anonymous functions](https://www.php.net/manual/en/functions.anonymous.php)
and [PHP arrow functions](https://www.php.net/manual/en/functions.arrow.php).
