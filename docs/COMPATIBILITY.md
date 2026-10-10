# PHP compatibility

PXR aims for **source compatibility with the shared subset of PHP**. For any
construct it implements, its **observable behaviour matches PHP**; for any
construct it does not implement yet, it **fails loudly at compile time** rather
than silently mis‑compiling.

This document lists (a) what PXR implements identically, (b) the few remaining
observable differences, and (c) the constructs that are not implemented yet
(which are compile errors, never silent divergences).

The canonical test is `examples/compat.php`, which is valid in both PHP 8 and PXR
and whose output the test suite asserts is byte‑identical.

## Implemented identically

| Behaviour | Notes |
| --- | --- |
| `<?php` tag, shebang, `//`, `#`, `/* */` comments | |
| `echo`, `print`, `if`/`elseif`/`else`, `while`, `for`, `do…while`, blocks | |
| `switch` (fall‑through, `break`/`continue` levels) | loose comparison; two numeric strings compare by value |
| `match` — strict `===` matching; `default` is a **fallback** wherever it appears | matches PHP |
| Ternary `? :` (right‑assoc), `??` (lazy), `<=>` | |
| `.` concatenation, `+ - * / %` | |
| `==`, `!=`, `===`, `!==`, `<`, `<=`, `>`, `>=` | loose comparisons (`==`, ordering, `<=>`) compare two numeric strings by value; `===`/`!==` are always byte equality |
| `&&`, `||`, `!` with truthiness and short‑circuit evaluation | |
| Bitwise `& | ^ ~`, shifts `<< >>` | PHP rules: count ≥ 64 → `0` (or `-1` for `>>` of a negative); negative count is a fatal `ArithmeticError` |
| `++` / `--`, compound assignment `+=` etc. | as *statements* only (see below) |
| String literals with escapes; hex / binary / `0o` octal / `_`‑separated ints | |
| Infrastructure: typed params, return types, recursion | types are **required** (see below) |
| Arrays `[...]` / `array(...)` | ordered maps, `int`/`string` keys, insertion order |
| **Array value semantics** | assignment, function arguments, returns and `foreach` all operate on copies (copy‑on‑write, realised as an eager deep copy) |
| **Array key unification** | a canonical decimal‑integer string key *is* the integer key: `$a["1"]` ≡ `$a[1]` (`"01"`, `"1.5"`, `"+1"`, `" 1"` stay strings) |
| `count`, `strlen`, `intval`, `floatval`, `strval`, `abs` | |
| Division `/` is always `float`; `%` is integer remainder | a zero divisor is a fatal `DivisionByZeroError` (exit 255), exactly as in PHP — not IEEE infinity |
| **Float stringification** (`echo`, `.`, `strval`) | PHP's `zend_gcvt` with precision 14: `0.1+0.2` → `"0.3"`, `1e15` → `"1.0E+15"`, `1e-5` → `"1.0E-5"`, `1.0` → `"1"`, `-0.0` → `"-0"`, `INF`/`-INF`/`NAN` | |
| `echo true` → `1`, `echo false` → *(nothing)*; `.` on `bool` → `"1"`/`""` | |
| Integral floats print without a decimal point (`1.0` → `"1"`) | |
| Function names case‑insensitive; variable names case‑sensitive | |

## Remaining observable differences

These are the **only** places where a program PXR accepts computes something
different from PHP. Both stem from the absence of a dynamic `null`/`int|float`
value type.

| Behaviour | PHP | PXR | Tracking |
| --- | --- | --- | --- |
| Integer overflow (`PHP_INT_MAX + 1`) | promotes to `float` | wraps (two's complement `int`) | needs an `int\|float` value type |
| Reading a missing array key | warning, value `null` | value `0` / `""` | needs `null`; `??` correctly detects absence |

Everything else that differs is a **loud compile error**, listed below.

## Not implemented yet — rejected at compile time

These are valid PHP; PXR refuses them with a specific diagnostic rather than
producing a different program. Each is a feature gap to be filled, not a
behavioural divergence.

| PHP feature | PXR status |
| --- | --- |
| Untyped parameters / missing return type | **Error** `E0202`, `E0204` — PXR requires types |
| Bare `array` type hint | **Error** — write the element type, `array<T>` (a PXR extension) |
| A variable changing type (`$x = 1; $x = "s";`) | **Error** — a variable has one static type |
| Double‑quoted interpolation `"$x"` | **Error** `E0008` — write `"a" . $x` |
| `null`, nullable types | **Error** `E0209` |
| Assignment as an expression (`$a = $b = 1`, `f($x++)`) | **Error** — `++`/`--`/`=` are statement‑only |
| Keyed `foreach ($a as $k => $v)` | **Error** `E0242` |
| `echo` / `print` an array, `(string)$array` | **Error** `E0240` — PHP prints `Array` *and* a warning to stdout, which PXR does not reproduce |
| Nested assignment `$a[0][1] = x` | **Error** `E0248` |
| `**` (exponentiation) | **Error** `E0257` |
| `+` with numeric strings (`"1" + 2`) | **Error** — convert with `intval()` / `floatval()` |
| Wrong argument type (non‑strict mode coercion) | **Error** `E0231` |
| Undefined variable / function | **Error** `E0220` / `E0232` |
| Leading‑zero octal literal (`0755`) | **Warning** `W0001`, read as decimal |
| Classes, interfaces, traits, enums, namespaces, closures | Not yet |
| `include`/`require`, `global`, `static`, references `&` | Not yet |
| Exceptions, generators, attributes | Not yet |

### Notes

**Array value semantics.** PHP arrays are value types: `$b = $a`, passing `$a`
to a function, returning and `foreach` all operate on a copy. PXR realises this
with an eager deep copy at every store/pass/return/iteration, so mutation
through one name is never visible through another. The current implementation
copies eagerly (not lazily); the observable result is identical. Strings are
immutable and shared, so they need no copy.

**Integer overflow.** `PHP_INT_MAX + 1` yields a `float` in PHP (and so does
negating `PHP_INT_MIN` — e.g. `abs(1 << 63)` — which PHP also promotes to
`float`). PXR's `int` is a fixed 64‑bit value and wraps on overflow. Faithfully
promoting to `float` requires the result of `int op int` to be a union type
(`int|float`), which is a language‑design change tracked for a later milestone.
Until then integer arithmetic is the one place a PXR result can differ, and it is
documented rather than silent.

**Numeric strings.** Comparison is numeric‑aware like PHP: `"10" < "9"` is
false, `"1" == "01"` is true, `"1" === "01"` is false, and `switch` follows the
same loose rule. `is_numeric_string` semantics are reproduced (surrounding
whitespace and a leading sign are allowed; hex, `inf`/`nan` and `_` separators
are not). Arithmetic, however, is **not** coerced: PXR requires numeric operands
and rejects `"1" + 2` at compile time; convert explicitly with `intval()` /
`floatval()`.

**Missing array keys.** PHP warns and yields `null`. PXR has no `null` yet, so a
direct read yields the zero value; `??` / `??=` detect absence correctly.

**Fail loud, never silent.** Because classes, `null`, keyed `foreach`, nested
assignment, interpolation, etc. are not implemented, using them is a compile
error with a specific code — never a silently different program.

## Runtime independence

A compiled PXR executable links only the system C library (verified with
`otool -L` on macOS). It does **not** embed or require the PHP interpreter, Zend
engine, or any PHP extension. See [`RUNTIME.md`](RUNTIME.md).
