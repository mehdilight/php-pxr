# Mixed values and guard narrowing — M4.1–M4.3

`mixed` is an explicit opt-in to runtime value tags. Concrete variables and
functions retain their existing static checks. The current implementation supports `int`,
`float`, `bool`, byte-string values, and `null`; M4 remains in progress.

```php
<?php
function boxed(mixed $value): mixed { return $value; }

function describe(mixed $value): string {
    if (is_int($value)) {
        return "integer: " . ($value + 1);
    }
    return strval($value);
}

echo describe(42) . "\n";
$value = boxed(1);
$value = "now a string";
echo $value . "\n";
```

These declarations and expressions also run under PHP. Local PXR annotations
such as `$value: mixed = 1;` provide another opt-in, but are a PXR extension.
Function parameter and return annotations remain required.

## What works

* Scalars are boxed automatically when assigned/passed/returned to a `mixed`
  destination. Mixed variables can subsequently hold a different supported value type.
  Parameters, returns, instance/static properties, and constructor arguments use
  the same representation rules. Constant scalar and null property defaults are supported.
* `is_int`, `is_float`, `is_bool`, `is_string`, `is_scalar`, `is_array`,
  `is_object`, and `is_null` inspect values. Concrete arguments are evaluated even
  when their type makes the result statically known. Null satisfies `is_null`
  and fails `is_scalar`; arrays and objects cannot yet be boxed.
* `intval`, `floatval`, `strval`, and `boolval` explicitly produce concrete
  scalar values. Other typed builtins retain their existing argument requirements.
* `echo`, `print`, concatenation, interpolation, truthiness, `&&`, `||`, and `!`
  support mixed values. `.=` stores a newly boxed string without changing aliases
  to the previous boxed value.
* Strict `===`/`!==` compares runtime value tags and values. Strings compare by
  bytes, including embedded NULs; integers and floats remain distinct tags.
* `match` can compare a mixed subject to concrete scalar or null arm conditions.
  Branches can combine mixed expressions with scalar results. Heterogeneous
  branches without an existing mixed expression still need explicit boxing.
* `null` literals produce mixed values. `??` and `??=` evaluate the fallback
  only for null/unset values, preserving false, zero, and empty strings. Mixed
  instance/static properties and array elements support the same behavior.
* Arrays can hold boxed scalar/null values under the homogeneous element type `mixed`:
  `$values = [boxed(1), boxed("two")];`. Append, element replacement, copying,
  iteration, and existing array transforms preserve tags and value semantics.

## Limits and next steps

This slice does not box arrays or objects. `array<T>` is not implicitly
converted to `array<mixed>`; box its elements explicitly. Untyped function
inference, union types, nullable types, runtime PHP includes, and a migration
report are still planned.

Scalar predicates narrow local mixed variables and parameters. Null predicates
and strict comparisons with null also constrain surviving paths. Inside
`if (is_int($value))`, integer operations and typed calls can use `$value`
directly. Negation, surviving paths after early returns, `elseif`/`else`,
short-circuit boolean expressions, ternaries, and successful strict scalar
comparisons also propagate facts. Assignments invalidate old guards; reachable
branch joins retain only facts valid on every surviving path. Guarded numeric
compound assignments and increments preserve mixed storage while updating the
known scalar result type.

Loop analysis discards facts for values that can be rebound across iterations.
While/for guards can establish new facts for each body execution. It is
conservative: a safe operation may still require another guard or an explicit
conversion if its type cannot be proved across loop paths. Guards currently
refine local slots, not repeated property or array-element reads; copy such a
value into a local variable before guarding it.

Unguarded mixed arithmetic, ordering, and loose comparisons are rejected with
`E0274`; proven concrete scalar values use the existing typed operators.
Implicit conversion from mixed to a concrete parameter/return/property type is
also rejected; use the explicit conversion functions. Read potentially missing
mixed array keys through `??`; direct missing-key reads have no valid box and
fail when used as mixed values. Typed operations such as
`strlen` require a concrete argument (`strlen(strval($value))`). Numeric string
conversions reuse the existing runtime's conversion semantics; this slice does
not claim full PHP dynamic-coercion compatibility.

The boxes are immutable and use the existing process arena; memory reclamation
remains M6. Compiled mixed programs run independently of PHP.

See [`examples/nulls.php`](../examples/nulls.php) for null storage, conversions,
strict matching, and lazy coalescing. A variable inferred from null has type
`mixed`, so it can later hold a supported scalar. Concrete scalar/class types
remain non-nullable; nullable type annotations are still planned.
