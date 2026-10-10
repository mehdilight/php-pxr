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
| Arrays, `[...]` | Supported (homogeneous element type, `int`/`string` keys) |
| `array(...)` literal | Supported |
| `for`, `foreach`, `do…while`, `switch`, `match`, `break`, `continue` | Supported (`foreach` value‑only; `match` needs `default`) |
| Ternary `? :`, null coalescing `??`, spaceship `<=>` | Supported |
| `++` / `--`, compound assignment `+=` etc. | Supported (as *statements* only) |
| Bitwise `& | ^ ~ << >>` | Supported (`int` operands only) |
| Assignment as an expression (`$a = $b = 1`, `f($x++)`) | **Error** (PXR has no assignment expressions) |
| Keyed `foreach ($a as $k => $v)` | **Error** `E0242` (not yet) |
| `echo`/`print` an array | **Error** `E0240` (not yet) |
| Nested assignment `$a[0][1] = x` | **Error** `E0248` (not yet) |
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
| Array assignment / passing to a function | Copy‑on‑write (by value) | **Reference** (a pointer is shared) |
| Reading a missing array key | Warning, value `null` | Value `0`/`""` (no `null` yet); `??` detects absence |
| Array keys | int and string unified (`"1"` ≡ `1`) | int and string keys are distinct |
| `match` with no arm matching | Throws `\UnhandledMatchError` | **Compile error** `E0256`: a `default` is required |
| `match` `default` placement | Evaluated in source order (a leading `default` always wins) | Acts purely as a fallback, wherever it appears |
| `>>` on a negative `int` | Arithmetic shift | Arithmetic shift (same) |

### Notes on the notable differences

**Integer overflow.** PHP silently promotes to `float`. PXR milestone 0 uses
fixed 64‑bit `int` and wraps on overflow, matching C semantics. This is an
explicit, documented incompatibility; a later milestone will either introduce a
checked overflow mode or a `float` promotion option. `pxr check` cannot detect it
for now.

**Numeric strings.** PHP's `+` coerces numeric strings. PXR requires numeric
operands for `+` and rejects `"1" + 2` at compile time. Convert explicitly with
`intval()`/`floatval()`.

**Missing constructs fail loudly.** Because classes, `null`, keyed `foreach`,
nested array assignment, etc. are not implemented, using them produces a compile
error with a specific code — never a silently different program.

**Arrays are references, not values.** PHP arrays are value types with
copy‑on‑write: assigning `$b = $a` or passing `$a` to a function makes a copy, so
mutating the copy does not affect the original. PXR arrays are represented as a
shared reference (an opaque pointer), so `$b = $a` makes `$a` and `$b` alias the
same storage and a mutation through one is visible through the other. This
difference is observable; keep array mutation local, or copy explicitly
(element by element) until value semantics land. `examples/quicksort.php` sorts
in place, so it behaves identically under both.

**Arithmetic shifts.** PXR masks the shift amount to `0..63` and uses an
arithmetic shift for `>>`, matching PHP for the common cases. PHP's behaviour for
oversized shift counts differs but is rarely relied upon.

## Runtime independence

A compiled PXR executable links only the system C library (verified with
`otool -L` on macOS). It does **not** embed or require the PHP interpreter, Zend
engine, or any PHP extension. See [`RUNTIME.md`](RUNTIME.md).
