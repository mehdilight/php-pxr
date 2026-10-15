# Examples

Run any of these with `pxr run <file>`, or from a release build:
`./target/release/pxr run examples/hello.php`.

| File | Shows | Valid PHP? |
| --- | --- | --- |
| `hello.php` | The smallest program | ✅ |
| `fib.php` | Recursion, typed functions, `while`, `if` | ⚠️ uses `$i: int` (PXR annotation) |
| `fizzbuzz.php` | Branching, `%`, `===`, concatenation | ⚠️ uses `$i: int` (PXR annotation) |
| `types.php` | All scalar types, widening, builtins | ❌ PXR-only annotations |
| `compat.php` | The shared PHP∩PXR subset; output is asserted identical to PHP | ✅ |
| `quicksort.php` | Arrays, `for`/`while`/`foreach`, `continue`, index assignment | ✅ |
| `objects.php` | Classes, properties, methods, constructors, visibility | ✅ |
| `inheritance.php` | Inheritance, overriding, `parent::`, `instanceof` | ✅ |
| `abstract.php` | Abstract classes, abstract methods, polymorphic dispatch | ✅ |
| `interfaces.php` | Interface contracts, multiple interface inheritance, dispatch, `instanceof` | ✅ |
| `static.php` | Static properties/methods, inheritance, `self::`, `parent::`, array copying | ✅ |
| `namespaces.php` | Namespace blocks, aliases, qualified names, function fallback | ✅ |
| `modules.php` | Compile-time dependencies in `modules/`, aliases, cross-file types | ✅ |
| `containers.php` | Mixed arrays/objects, value copies, identity, guards, dynamic operators | ✅ |
| `migration/main.php` | Native catalog totals with legacy PHP CSV I/O and persistent state | ✅ |
| `mixed.php` | Tagged scalar values, predicates, conversions, properties, strict comparisons | ✅ |
| `nulls.php` | Boxed nulls, predicates, conversions, strict matching, lazy coalescing | ✅ |
| `narrowing.php` | Scalar guards, early returns, short-circuiting, guarded mixed mutation | ✅ |
| `generics.php` | Generic functions, explicit/inferred calls, native specialization, recursion | ❌ PXR generic syntax |
| `generic_classes.php` | Generic classes, function/class inference, nested types, array copies, identity | ❌ PXR generic syntax |
| `generic_methods.php` | Independent method parameters, static calls, parent calls, generic interface methods | ❌ PXR generic syntax |
| `constraints.php` | Nominal generic bounds and interface intersections on functions/classes/methods | ❌ PXR generic syntax |
| `traits.php` | Nested trait composition, private properties, and static state | ✅ |
| `inference.php` | Inferred returns, recursion, numeric joins, generic function/method results | ❌ PXR generic syntax |

`compat.php` and `quicksort.php` are compatibility proofs: they compile with
`pxr` and run under `php`, and both produce the same bytes.

```sh
php examples/compat.php
pxr run examples/compat.php

php examples/quicksort.php
pxr run examples/quicksort.php
```

## Ownership and collection

`ownership.php` demonstrates `owned<T>`, generic transfer, object/array borrows,
and explicit drops. Its PXR extension syntax does not parse in PHP.
`memory.php` uses ordinary shared objects to create and discard 10,000 cycles:

```sh
pxr run examples/ownership.php
PXR_GC_STATS=1 pxr run examples/memory.php
```

See [`OWNERSHIP.md`](../docs/OWNERSHIP.md) for the static rules and supported scope.

## Seeing an error

Compile-time diagnostics are the point. Try:

```sh
printf '<?php echo $undefined;\n' > /tmp/bad.php
pxr check /tmp/bad.php
```

```
/tmp/bad.php: error: [E0220] undefined variable `$undefined`
       |
    1 | echo $undefined;
       |      ^^^^^^^^^^
       = note: variables must be assigned before they are used
```
