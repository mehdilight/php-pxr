# Scalar mixed values — M4.1

`mixed` is an explicit opt-in to runtime scalar tags. Concrete variables and
functions retain their existing static checks. This first slice supports `int`,
`float`, `bool`, and byte-string values; M4 remains in progress.

```php
<?php
function boxed(mixed $value): mixed { return $value; }

function describe(mixed $value): string {
    if (is_int($value)) {
        return "integer: " . intval($value);
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
  destination. Mixed variables can subsequently hold a different scalar type.
  Parameters, returns, instance/static properties, and constructor arguments use
  the same representation rules. Constant scalar property defaults are supported.
* `is_int`, `is_float`, `is_bool`, `is_string`, `is_scalar`, `is_array`,
  `is_object`, and `is_null` inspect values. Concrete arguments are evaluated even
  when their type makes the result statically known. Boxed values in this slice
  always satisfy `is_scalar`; they never satisfy the last three predicates.
* `intval`, `floatval`, `strval`, and `boolval` explicitly produce concrete
  scalar values. Other typed builtins retain their existing argument requirements.
* `echo`, `print`, concatenation, interpolation, truthiness, `&&`, `||`, and `!`
  support mixed values. `.=` stores a newly boxed string without changing aliases
  to the previous boxed value.
* Strict `===`/`!==` compares runtime scalar tags and values. Strings compare by
  bytes, including embedded NULs; integers and floats remain distinct tags.
* `match` can compare a mixed subject to concrete scalar arm conditions.
  Branches can combine mixed expressions with scalar results. Heterogeneous
  branches without an existing mixed expression still need explicit boxing.
* Arrays can hold boxed scalars under the homogeneous element type `mixed`:
  `$values = [boxed(1), boxed("two")];`. Append, element replacement, copying,
  iteration, and existing array transforms preserve tags and value semantics.

## Limits and next steps

This slice does not box arrays, objects, or null. `array<T>` is not implicitly
converted to `array<mixed>`; box its elements explicitly. Untyped function
inference, union types, nullable types, runtime PHP includes, and a migration
report are still planned.

Predicates currently return booleans without changing a variable's static type.
Even inside `if (is_int($value))`, arithmetic requires `intval($value)`. Sound flow
narrowing is the next M4 slice, including invalidation after assignments and
merging information across branches and loops.

Mixed arithmetic, ordering, and loose comparisons are rejected with `E0274`.
Implicit conversion from mixed to a concrete parameter/return/property type is
also rejected; use the explicit conversion functions. Read potentially missing
mixed array keys through `??`; direct missing-key reads have no scalar box and
fail when used as mixed values. Typed operations such as
`strlen` require a concrete argument (`strlen(strval($value))`). Numeric string
conversions reuse the existing runtime's conversion semantics; this slice does
not claim full PHP dynamic-coercion compatibility.

The boxes are immutable and use the existing process arena; memory reclamation
remains M6. Compiled mixed programs run independently of PHP.
