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

`compat.php` and `quicksort.php` are compatibility proofs: they compile with
`pxr` and run under `php`, and both produce the same bytes.

```sh
php examples/compat.php
pxr run examples/compat.php

php examples/quicksort.php
pxr run examples/quicksort.php
```

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
