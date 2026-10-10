# PHP compatibility

PXR aims for **source compatibility with the shared subset of PHP**, and where it
cannot preserve PHP's observable behaviour it prefers to **fail at compile time**
rather than silently mis‑compile. This document lists the concrete differences.

The canonical test is `examples/compat.php`, which is valid in both PHP 8 and PXR
and whose output the test suite asserts is byte‑identical.

## Grammar and syntax

| PHP feature | PXR status |
| --- | --- |
| `<?php` tag, shebang, `//`, `#`, `/* */` comments | Supported |
| `echo`, `print`, `if`/`elseif`/`else`, `while`, blocks, `return` | Supported |
| Typed parameters `function f(int $x)` and return types `: int` | Supported (types **required**) |
| Untyped parameters / missing return type | **Error** (`E0202`, `E0204`) |
| Variables `$x`, assignment, `int`→`float` widening | Supported |
| `.` concatenation, `+ - * / %` | Supported |
| `==`, `!=`, `===`, `!==`, `<`, `<=`, `>`, `>=` | Supported |
| `&&`, `||`, `!` with truthiness and short‑circuiting | Supported |
| String literals with escapes | Supported (single/double quotes) |
| Hex / binary / `0o` octal / underscore‑separated integers | Supported |
| Leading‑zero octal (`0755`) | **Warning** `W0001`, read as decimal |
| Double‑quoted interpolation `"$x"` | **Warning** `W0002`, kept literal (not yet) |
| `null`, nullable types | **Error** `E0209` (not yet) |
| Arrays, `[...]`, `array()` | Not yet |
| `for`, `foreach`, `do…while`, `switch`, `match`, `break`, `continue` | Not yet |
| Ternary `? :`, null coalescing `??`, spaceship `<=>` | Not yet |
| `++` / `--`, compound assignment `+=` etc. | Not yet |
| Bitwise `& | ^ ~ << >>` | **Error** `E0003` (not yet) |
| Classes, interfaces, traits, enums, namespaces, closures | Not yet |
| `include`/`require`, `global`, `static`, references `&` | Not yet |
| Exceptions, generators, attributes | Not yet |

## Semantics

| Behaviour | PHP | PXR |
| --- | --- | --- |
| Undefined variable | Warning, value `null` | **Compile error** `E0220` |
| Undefined function | Fatal at call time | **Compile error** `E0232` |
| Wrong argument type (non‑strict mode) | Coerced | **Compile error** `E0231` |
| Integer overflow (`PHP_INT_MAX + 1`) | Promotes to `float` | **Wraps** (two's complement) |
| `+` on numeric strings | Numeric add | **Compile error** (no string→number in `+`) |
| `.` on `bool` | `"1"` / `""` | Same |
| `echo true;` / `echo false;` | `1` / *(nothing)* | Same |
| Division `/` | Always `float` | Same |
| `%` | Integer remainder | Integer remainder (`int` only) |
| Loose `==` numeric string vs number | Numeric coercion rules | **Compile error** unless same kind |
| Function name case | Insensitive | Insensitive |
| Variable name case | Sensitive | Sensitive |
| Float printing (e.g. `1.0`) | `"1"` | `"1"` (integral floats print without a point) |

### Notes on the notable differences

**Integer overflow.** PHP silently promotes to `float`. PXR milestone 0 uses
fixed 64‑bit `int` and wraps on overflow, matching C semantics. This is an
explicit, documented incompatibility; a later milestone will either introduce a
checked overflow mode or a `float` promotion option. `pxr check` cannot detect it
for now.

**Numeric strings.** PHP's `+` coerces numeric strings. PXR requires numeric
operands for `+` and rejects `"1" + 2` at compile time. Convert explicitly with
`intval()`/`floatval()`.

**Missing constructs fail loudly.** Because arrays, `null`, `for`/`foreach`, etc.
are not implemented, using them produces a compile error with a specific code —
never a silently different program.

## Runtime independence

A compiled PXR executable links only the system C library (verified with
`otool -L` on macOS). It does **not** embed or require the PHP interpreter, Zend
engine, or any PHP extension. See [`RUNTIME.md`](RUNTIME.md).
