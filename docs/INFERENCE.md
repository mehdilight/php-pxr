# Return-type inference

M5 infers omitted return annotations for native function and concrete method
bodies. Parameters still require annotations. Explicit return annotations retain
their existing checks and coercions.

```php
function twice(int $value) { return $value * 2; } // int
function number(bool $whole) {
    if ($whole) { return 1; }
    return 2.5;
} // float; the integer branch widens at return
function identity<T>(T $value) { return $value; } // concrete T per specialization
```

Inference considers the whole body and iterates across calls, including forward
references, methods, and direct/mutual recursion. A concrete base case or an
annotated dependency can supply the evidence needed by a recursive cycle:

```php
function fib(int $n) {
    if ($n < 2) { return $n; }
    return fib($n - 1) + fib($n - 2);
} // int
```

Generic functions and independent generic methods infer a separate concrete
result for each instantiation. Inference and class/method specialization finish
together before IR generation. When a provisional result changes, obsolete
specializations are discarded. Overrides and interface implementations are
checked against their final inferred result, preserving their native ABI.

## Joining return paths

* Identical types stay concrete; scalar `int` and `float` join to `float`.
* No value returns, or only bare returns, infer `void`. A function returning a
  value must return on every path; fallthrough does not silently add null.
* A mixed return path joins other value paths to `mixed`, with normal boxing.
  Null literals already have mixed type. This does not add nullable annotations.
* Named results can join to a common class ancestor. An explicit interface
  annotation can express a contract when no common class result is inferred.
* Empty arrays join with a compatible concrete array. Array storage layouts
  must remain compatible: `array<int>` and `array<float>` are not widened into
  one array type. Use an explicit mixed boundary when boxing is intended.

Unrelated concrete scalar results, incompatible arrays, and value/bare-return
mixtures need an explicit appropriate annotation or a body change. The compiler
does not invent union types, reinterpret element storage, or silently opt every
ambiguous result into mixed.

An unconstrained cycle such as `function f() { return f(); }` has no evidence
for a result type and fails with `E0284`. An annotation can resolve such a cycle.
Expanding recursive result types also fail: inference is bounded to 64 updates
per result, 64 nested array levels, and 2,048 analysis rounds. These are compile
limits, not runtime behavior. Unresolved types never reach native lowering.

Abstract and interface signatures have no body to infer from and still require
return annotations (`E0204`). Constructors retain their implicit void result.
Inference follows the compiler's existing conservative flow analysis; it does
not promise exhaustive proofs of loop termination or unreachable branches.

See [`../examples/inference.php`](../examples/inference.php).
