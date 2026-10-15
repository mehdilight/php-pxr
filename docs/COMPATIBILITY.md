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
| **Double‑quoted interpolation** `"$name"`, `"{$name}"` | equivalent to `"" . $name`; `$` not followed by a name start stays literal (`"$"`, `"$1"`, `"foo$"`, `"$$x"`); the array, property and `${...}` forms are rejected (see below) |
| Infrastructure: typed params, return types, recursion | types are **required** (see below) |
| Arrays `[...]` / `array(...)` | ordered maps, `int`/`string` keys, insertion order |
| **Array value semantics** | assignment, function arguments, returns and `foreach` all operate on copies (copy‑on‑write, realised as an eager deep copy) |
| **Array key unification** | a canonical decimal‑integer string key *is* the integer key: `$a["1"]` ≡ `$a[1]` (`"01"`, `"1.5"`, `"+1"`, `" 1"` stay strings) |
| `count`, `strlen`, `intval`, `floatval`, `strval`, `abs` | |
| **String builtins** `strtoupper`, `strtolower`, `trim`/`ltrim`/`rtrim`, `substr`, `str_repeat`, `str_replace`, `implode`, `explode`, `str_contains`, `str_starts_with`, `str_ends_with`, `strrev`, `ucfirst`, `lcfirst`, `ucwords`, `str_split`, `str_pad`, `substr_count` | byte-exact vs PHP, embedded NULs safe; see notes below |
| **Escaping & encoding** `addslashes`, `stripslashes`, `bin2hex`, `base64_encode`, `urlencode`, `rawurlencode`, `urldecode`, `rawurldecode` | byte-exact vs PHP over all 256 byte values, including NULs; see notes below |
| **Formatted printing** `sprintf`, `printf` | byte-exact vs PHP; supports `%s`, `%d`/`%i`, `%u`, `%f`/`%F`, `%e`/`%E`, `%g`/`%G`, `%x`/`%X`, `%o`, `%b`, `%c`, `%%`, widths, precision, padding, alignment, positional arguments; see notes below |
| **Array transforms** `array_values`, `array_reverse`, `array_merge`, `array_slice`, `array_pad`, `array_fill`, `array_key_exists` | byte-exact vs PHP; deep-copy nested arrays (value semantics); see notes below |
| Division `/` is always `float`; `%` is integer remainder | a zero divisor is a fatal `DivisionByZeroError` (exit 255), exactly as in PHP — not IEEE infinity |
| **Float stringification** (`echo`, `.`, `strval`) | PHP's `zend_gcvt` with precision 14: `0.1+0.2` → `"0.3"`, `1e15` → `"1.0E+15"`, `1e-5` → `"1.0E-5"`, `1.0` → `"1"`, `-0.0` → `"-0"`, `INF`/`-INF`/`NAN` | |
| `echo true` → `1`, `echo false` → *(nothing)*; `.` on `bool` → `"1"`/`""` | |
| Integral floats print without a decimal point (`1.0` → `"1"`) | |
| Function names case‑insensitive; variable names case‑sensitive | |
| **Classes and objects** | `class`, typed properties with defaults, methods, constructors (`__construct`), visibility (`public`, `protected`, `private`), `$this`, method chaining, instantiation (`new`) |

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
| Complex interpolation: `"$a[0]"`, `"$a[$k]"`, `"{$a[0]}"`, `"$obj->p"`, `"${name}"` | **Error** `E0008` — simple `"$name"` / `"{$name}"` work; write these as a concatenation instead |
| `null`, nullable types | **Error** `E0209` |
| Assignment as an expression (`$a = $b = 1`, `f($x++)`) | **Error** — `++`/`--`/`=` are statement‑only |
| Keyed `foreach ($a as $k => $v)` | **Error** `E0242` |
| `echo` / `print` an array, `(string)$array` | **Error** `E0240` — PHP prints `Array` *and* a warning to stdout, which PXR does not reproduce |
| Nested assignment `$a[0][1] = x` | **Error** `E0248` |
| `**` (exponentiation) | **Error** `E0257` |
| `+` with numeric strings (`"1" + 2`) | **Error** — convert with `intval()` / `floatval()` |
| Heterogeneous arrays (mixed value types) | **Error** — PXR arrays are homogeneous (`array<T>`); e.g. `array_merge` of an `array<int>` and an `array<string>` is `E0253` |
| Wrong argument type (non‑strict mode coercion) | **Error** `E0231` |
| Undefined variable / function | **Error** `E0220` / `E0232` |
| Leading‑zero octal literal (`0755`) | **Warning** `W0001`, read as decimal |
| Functions that return `string|false` / `int|false` (`strpos`, `strstr`, `base64_decode`, `hex2bin`, `substr_compare`, …) | Not yet — needs a union / `false` value type; only the never-failing encoders above are provided |
| `number_format` | Not yet |
| Traits, enums, namespaces, closures | Not yet |
| `include`/`require`, `global`, function-local `static`, references `&` | Not yet |
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

**Byte strings.** PHP strings are byte strings and PXR's are too: `len` bytes,
so embedded `\0` bytes are first‑class. `strlen("a\0b")` is `3`, `===`/`!==` and
ordering compare all bytes, and concatenation preserves them. Escapes follow PHP
(`\n \t \r \v \f \e \\ \" \$ \0 \xHH`; single quotes only `\\` and `\'`); the
byte value of the sequence is what matters, so `"\x80"` is a one‑byte string.
This holds for *any* byte: `\xHH` for `HH ≥ 0x80` produces that raw byte, not a
Unicode replacement character (a regression test guards this).

**Interpolation.** A double‑quoted string interpolates `$name` and `{$name}` and
desugars to a `"" . …` concatenation, so the value is string‑converted exactly as
PHP's echo/concat conversion is (`5` → `"5"`, `3.5` → `"3.5"`, `true` → `"1"`,
`false` → `""`). A `$` that is not followed by a name start is a literal `$`, as
in PHP: `"$"`, `"$1"`, `"$ "`, `"foo$"` and `"$$x"` (→ `"$"` then `$x`) all match
byte‑for‑byte. `\$` is an escaped dollar. PHP's remaining interpolation forms are
rejected loudly rather than mis‑lexed: simple array access `"$a[0]"`/`"$a[$k]"`,
property access `"$obj->p"`, complex `"{$expr}"` (only `"{$name}"` is accepted),
and the deprecated `"${name}"` are all **Errors** `E0008`. Interpolating a value
with no string conversion (an array) is a loud type error (`E0223`), never PHP's
`"Array"`-plus-warning.

**String builtins.** `strtoupper`/`strtolower` map bytes only (ASCII `a–z`/`A–Z`),
exactly like PHP's locale-independent conversion: `"héllo"` → `"héllo"`. `trim`
(default mask `` " \t\n\r\0\x0B" ``, replaceable by a byte mask; an *explicit*
empty mask strips nothing, matching PHP `trim(" x ", "")` → `" x "`). `substr`
follows PHP 8 exactly — offsets clamp, a negative length cuts from the end, and
out-of-range results are the empty string, never `false`. `str_repeat` with a
negative count raises PHP's fatal `ValueError` (exit 255). `str_replace` accepts
string arguments with PHP's non-overlapping left-to-right scan (replacement text
is never rescanned); the `array` and by-reference `&$count` forms are **Errors**
`E0234`/`E0233`. `implode` accepts `(array)` or `(separator, array)` and coerces
scalar elements like PHP (`1.5` → `"1.5"`, `true` → `"1"`, `false` → `""`);
arrays of arrays are an **Error** `E0234`. `explode` supports positive limits
(remainder is the final piece), `0` (treated as `1`) and negative limits
(dropping trailing pieces exactly like PHP), returns `array<string>`, and an
empty separator raises PHP's fatal `ValueError` (exit 255).

`str_contains`/`str_starts_with`/`str_ends_with` are case-sensitive and
NUL-safe; the empty needle/prefix/suffix is always found. `strrev` reverses
*bytes* (`"héllo"` becomes `"oll\xA9\xC3h"`). `ucfirst`/`lcfirst`/`ucwords`
change only the relevant byte and only for ASCII letters, like PHP's
locale-independent helpers; `ucwords` splits on the default whitespace
delimiters `" \t\n\r\f\v"`. `str_split` chunks into at-most-`length` byte pieces
(default `1`), returns `[]` for `""` (PHP 8.2+ semantics), and a `length < 1`
raises PHP's fatal `ValueError` (exit 255). `array_values` drops keys and
reindexes; `array_reverse` reverses order, renumbering integer keys and keeping
string keys (or keeping all keys when the second argument is `true`). Both
deep-copy nested arrays so value semantics hold. All of these are verified
byte‑for‑byte against PHP 8.4, including embedded-NUL cases.

**Escaping & encoding.** `addslashes` backslash-escapes `'`, `"` and `\` and
turns a NUL byte into the two characters `\0`; `stripslashes` is PHP's exact
inverse — it drops each backslash and emits the following byte, mapping `\0` back
to a NUL (so, as in PHP, `\n` becomes `n` and a trailing backslash is dropped).
`bin2hex` is lowercase, two digits per byte. `base64_encode` is standard RFC 4648
with `=` padding. The URL codecs follow PHP's byte tables exactly: `urlencode`
passes `A-Za-z0-9-_.`, turns a space into `+` and percent-encodes everything else
in uppercase hex (including `~`); `rawurlencode` passes `~` too and emits `%20`
for a space; `urldecode` maps `+` to a space while `rawurldecode` does not; a `%`
not followed by two hex digits is left literally, as in PHP. All eight are
verified byte‑for‑byte against PHP over inputs spanning all 256 byte values,
including high bytes and NULs, and `urldecode(urlencode($x))` /
`rawurldecode(rawurlencode($x))` round-trip identically.

**Formatted printing.** `sprintf` and `printf` reproduce PHP's formatting
engine: conversion specifiers `%s`, `%d`/`%i`, `%u`, `%f`/`%F`, `%e`/`%E`,
`%g`/`%G`/`%h`/`%H`, `%x`/`%X`, `%o`, `%b`, `%c`, and literal `%%`. Modifiers
include positional arguments (`%2$s %1$s`), sign prefix (`%+d`), padding
(spaces, zeros, or custom character `%'*10s`), left-alignment (`%-10s`), width,
and precision (`%.2f`, `%.3s`). Scalar arguments (`int`, `float`, `string`,
`bool`) are coerced matching PHP's type rules. Missing arguments raise PHP's
fatal `ArgumentCountError` (exit 255); unknown format specifiers or dangling
percent signs raise fatal `ValueError` (exit 255). Verified byte-for-byte
against PHP 8.4.

**Array builtins.** `array_merge` renumbers integer keys in order and lets a
later string key overwrite an earlier one in place; `array_slice` reproduces
PHP's offset/length arithmetic (negative offsets count from the end, a negative
length stops that many before the end, out-of-range clamps) and keeps string keys
while renumbering integer keys unless `$preserve_keys` is `true`. `array_pad`
pads to `|$size|` (prepend when negative) and, when no padding is needed, returns
the array unchanged with its keys; `array_fill` uses keys
`$start .. $start+$count-1` and a negative count raises PHP's fatal `ValueError`
(exit 255). `array_key_exists` accepts `int`/`string` keys and unifies canonical
integer strings, so `array_key_exists("1", $a)` matches the integer key `1`.
`str_pad`'s pad type is the integer `STR_PAD_*` value (`0`/`1`/`2` =
left/right/both, default `1`); each padded side cycles the pad string from its
start, and an empty pad string or an invalid pad type raises PHP's fatal
`ValueError`. `substr_count` counts non-overlapping occurrences and reproduces
PHP's argument checks *in their exact order* (empty needle, then offset, then
length) and their fatal messages. All of these are verified byte-for-byte against
PHP 8.4.

**Homogeneous arrays.** PXR arrays have a single static element type, so PHP's
*heterogeneous* arrays have no PXR representation. `array_merge` of two arrays
with different element types (`array<int>` with `array<float>` or `array<string>`,
etc.) is therefore a loud `E0253` error, as is `array_pad` with a value that is
not assignable to the element type. An empty literal `[]` (element type `null`)
unifies with any element type. See the "Not implemented yet" table.

**Missing array keys.** PHP warns and yields `null`. PXR has no `null` yet, so a
direct read yields the zero value; `??` / `??=` detect absence correctly.

**Classes, inheritance & polymorphism.** Single inheritance (`extends`), method
overriding, `parent::__construct` / `parent::method`, member visibility (`public`,
`protected`, `private`), and `instanceof` are fully supported with 100% byte-for-byte
output fidelity to PHP 8.4. Polymorphic method calls dynamically dispatch to subclass
overrides via runtime class tags.

**Abstract classes and methods.** Abstract classes can contain concrete methods,
typed properties, and abstract signatures. Concrete descendants must implement
all abstract methods with compatible signatures and visibility. Instantiation of
abstract classes, private abstract methods, and `parent::` calls to abstract
methods are compile-time errors (`E0269`). As with other PXR methods, parameter
and return types are required (constructors may omit their return type).

**Interfaces.** Interfaces support public typed method signatures, multiple
interface inheritance, and multiple class implementation contracts. Calls through
interface types and `instanceof` preserve interface membership inherited from
parent classes and parent interfaces. Abstract classes may defer implementations;
concrete classes must provide compatible public methods, including constructors.
Parameters must have identical types in this initial subset (PHP permits broader
contravariant overrides); named return types may narrow covariantly. Numeric
`int` to `float` widening is not valid signature covariance. Constants and interface property declarations are not implemented. Static
interface methods are supported with matching static implementations.

**Static properties and methods.** Static members support visibility, inheritance,
class-qualified access, lexical `self::`, and `parent::`. Inherited static
properties share their declaring class's storage; redeclaring one in a subclass
creates separate storage. Static methods have no `$this`. Static properties must
have constant scalar/array defaults in this slice; missing defaults are rejected
instead of silently treating an uninitialized typed property as zero. Array
properties can be read, copied, and replaced; direct element assignment remains
unsupported as it is for other property arrays. Object-style calls to static
methods and late static binding (`static::`) are rejected with `E0271`.
Function-local static variables and dynamic class/member names remain unsupported.

**Fail loud, never silent.** Features not yet implemented (such as
`null`, keyed `foreach`, nested array assignment, complex interpolation forms)
produce a compile error with a specific code — never a silently different program.

## Runtime independence

A compiled PXR executable links only the system C library (verified with
`otool -L` on macOS). It does **not** embed or require the PHP interpreter, Zend
engine, or any PHP extension. See [`RUNTIME.md`](RUNTIME.md).
