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

`compat.php` is the compatibility proof: it compiles with `pxr` and runs under
`php`, and both produce the same bytes.

```sh
php examples/compat.php
pxr run examples/compat.php
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
