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
| **Double‑quoted interpolation** `"$name"`, `"{$name}"` | equivalent to `"" . $name`; `$` not followed by a name start stays literal (`"$"`, `"$1"`, `"foo$"`, `"$$x"`); array/property access and braced access expressions work; deprecated `${...}` is rejected (see below) |
| Infrastructure: typed params, return types, recursion | parameter annotations required; concrete-body returns inferred when omitted |
| Arrays `[...]` / `array(...)` | ordered maps, `int`/`string` keys, insertion order |
| **Array value semantics** | assignment, function arguments, returns and `foreach` all operate on copies (copy-on-write storage) |
| **Array key unification** | a canonical decimal‑integer string key *is* the integer key: `$a["1"]` ≡ `$a[1]` (`"01"`, `"1.5"`, `"+1"`, `" 1"` stay strings) |
| `count`, `strlen`, `intval`, `floatval`, `strval`, `abs` | |
| **String builtins** `strtoupper`, `strtolower`, `trim`/`ltrim`/`rtrim`, `substr`, `str_repeat`, `str_replace`, `implode`, `explode`, `str_contains`, `str_starts_with`, `str_ends_with`, `strrev`, `ucfirst`, `lcfirst`, `ucwords`, `str_split`, `str_pad`, `substr_count` | byte-exact vs PHP, embedded NULs safe; see notes below |
| **Escaping & encoding** `addslashes`, `stripslashes`, `bin2hex`, `base64_encode`, `urlencode`, `rawurlencode`, `urldecode`, `rawurldecode` | byte-exact vs PHP over all 256 byte values, including NULs; see notes below |
| **Formatted printing** `sprintf`, `printf` | byte-exact vs PHP; supports `%s`, `%d`/`%i`, `%u`, `%f`/`%F`, `%e`/`%E`, `%g`/`%G`, `%x`/`%X`, `%o`, `%b`, `%c`, `%%`, widths, precision, padding, alignment, positional arguments; see notes below |
| **Array transforms** `array_values`, `array_reverse`, `array_merge`, `array_slice`, `array_pad`, `array_fill`, `array_key_exists` | byte-exact vs PHP; copy nested array values with copy-on-write storage; see notes below |
| Division `/` is always `float`; `%` is integer remainder | a zero divisor is a fatal `DivisionByZeroError` (exit 255), exactly as in PHP — not IEEE infinity |
| **Float stringification** (`echo`, `.`, `strval`) | PHP's `zend_gcvt` with precision 14: `0.1+0.2` → `"0.3"`, `1e15` → `"1.0E+15"`, `1e-5` → `"1.0E-5"`, `1.0` → `"1"`, `-0.0` → `"-0"`, `INF`/`-INF`/`NAN` | |
| `echo true` → `1`, `echo false` → *(nothing)*; `.` on `bool` → `"1"`/`""` | |
| Integral floats print without a decimal point (`1.0` → `"1"`) | |
| Function names case‑insensitive; variable names case‑sensitive | |
| **Classes and objects** | `class`, typed properties with defaults, methods, constructors (`__construct`), visibility (`public`, `protected`, `private`), `$this`, method chaining, instantiation (`new`) |

## Remaining observable differences

Concrete code retains the following differences. Mixed runtime tags handle
null and promoted numeric results without changing the ABI of concrete values.
The explicit interpreter boundary has additional scope/data rules below.

| Behaviour | PHP | PXR | Tracking |
| --- | --- | --- | --- |
| Integer overflow (`PHP_INT_MAX + 1`) | promotes to `float` | concrete `int` wraps; mixed promotes | concrete ABI remains fixed-width |
| Reading a missing array key | warning, value `null` | value `0` / `""` | mixed reads fail without a box; use `??` |

Unsupported native syntax produces compile errors. Opted-in mixed operand and
interop errors can also fail at runtime. Native warning text and diagnostic
formatting are not a complete emulation of PHP configuration.

## Not implemented yet — rejected at compile time

These are valid PHP; PXR refuses them with a specific diagnostic rather than
producing a different program. Each is a feature gap to be filled, not a
behavioural divergence.

| PHP feature | PXR status |
| --- | --- |
| Untyped parameters | **Error** `E0202`; returns can be inferred for concrete bodies |
| Missing abstract/interface return type | **Error** `E0204`; these signatures have no body to infer |
| Bare `array` type hint | **Error** — write the element type, `array<T>` (a PXR extension) |
| A concrete variable changing type (`$x = 1; $x = "s";`) | **Error** — opt into `mixed` to allow type changes |
| Deprecated `${name}` and invalid interpolation | **Error** `E0008`; simple array/property access and braced access expressions are supported |
| Nullable type annotations | Not yet; null literals work through mixed storage |
| Assignment as an expression (`$a = $b = 1`, `f($x++)`) | Supported for shared values; owned/borrowed binding duplication is rejected |
| Keyed `foreach ($a as $k => $v)` | Supported; key destination uses `mixed` (predeclared concrete destinations: `E0242`) |
| `echo` / `print` an array, `(string)$array` | **Error** `E0240` — PHP prints `Array` *and* a warning to stdout, which PXR does not reproduce |
| Nested array/property assignment | Supported, including COW snapshots, mixed null autovivification and suspended right-hand sides |
| `**` (exponentiation) | **Error** `E0257` |
| `+` with numeric strings (`"1" + 2`) | **Error** for concrete operands; numeric strings work through mixed operators |
| Heterogeneous arrays | Use mixed/array-of-mixed boundaries; unrelated homogeneous array types still cannot merge implicitly |
| Wrong argument type (non‑strict mode coercion) | **Error** `E0231` |
| Undefined variable / function | `E0220` / `E0232` in native-only code; unbound calls resolve at runtime when a PHP boundary is declared |
| Leading‑zero octal literal (`0755`) | **Warning** `W0001`, read as decimal |
| Functions that return `string|false` / `int|false` (`strpos`, `strstr`, `base64_decode`, `hex2bin`, `substr_compare`, …) | Not yet — needs a union / `false` value type; only the never-failing encoders above are provided |
| `number_format` | Not yet |
| Enums, reference closures | Not yet; native traits and by-value/bound/static closures, including generic scopes, are supported |
| Runtime include/require | Supported for explicitly marked PHP files; see [interop](INTEROP.md) |
| `global`, function-local `static`, PHP reference aliases | Available inside PHP boundaries; compiled syntax remains restricted |
| Exceptions, generators, attributes | Not yet |

### Notes

**Array value semantics.** PHP arrays are value types: `$b = $a`, passing `$a`
to a function, returning and `foreach` all operate on a copy. PXR realises this
with a copy-on-write array header at every store/pass/return/iteration. Copies
share backing storage until a write separates it, so mutation through one name
is never visible through another. Nested array values follow the same rule;
objects keep reference identity. Strings are immutable and shared.

**Integer overflow.** `PHP_INT_MAX + 1` yields a `float` in PHP (and so does
negating `PHP_INT_MIN` — e.g. `abs(1 << 63)` — which PHP also promotes to
`float`). PXR's `int` is a fixed 64‑bit value and wraps on overflow. Faithfully
promoting to `float` requires the result of `int op int` to be a union type
(`int|float`), which is a language‑design change tracked for a later milestone.
Mixed arithmetic now supplies runtime integer/float tags and promotes overflow;
concrete `int` operations retain their fixed-width ABI.

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
byte‑for‑byte. `\$` is an escaped dollar. Simple array accesses (`"$a[0]"`, `"$a[$k]"`, `"$a[name]"`), property access
(`"$obj->p"`), and braced array/property/method access (`"{$a['name'][0]}"`,
`"{$obj->label(1)}"`) work. Simple bare keys are string keys. Deprecated
`"${name}"` and invalid interpolation are **Errors** `E0008`. Interpolating a value
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
copy nested array values with copy-on-write storage so value semantics hold. All
of these are verified byte‑for‑byte against PHP 8.4, including embedded-NUL cases.

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

**Missing array keys.** PHP warns and yields `null`. A direct read from a concrete array yields its zero value; a direct missing
mixed read fails when used because it has no box. `??` / `??=` detect missing
keys and actual null values correctly.

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
types are required for parameters and abstract/interface returns. Concrete
method returns can be inferred; constructors retain implicit void.

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

**Namespaces and modules.** Braced and unbraced namespaces, qualified names,
class/namespace aliases, and `use function` imports are supported. Unqualified
function calls fall back to global functions and builtins; qualified function
names and class references do not. Imports apply only to their file and namespace
block. Grouped imports, constant imports, autoloading, and dynamic names remain
unsupported.

Unmarked literal includes/requires load declarations at compile time and precede
native executable statements. Leading `// @pxr-dynamic` marks legacy files for
runtime PHP include/include_once/require/require_once, including conditional and
function-local includes. Calls into legacy PHP return mixed values. Native paths
resolve from the containing file; legacy dependencies follow PHP's own rules.
See [`MODULES.md`](MODULES.md) and [`INTEROP.md`](INTEROP.md).

**Mixed values.** Mixed annotations accept null, scalars, arrays, and compiled
objects or PHP handles. Arrays retain value semantics; objects retain identity.
Predicates and scalar/array/instanceof guards refine locals and parameters;
assignments and joins invalidate stale facts. Native `__toString` dispatch and
PHP handle stringification are supported. Concrete destinations require a guard
or explicit conversion.

Dynamic arithmetic, numeric strings, overflow promotion, array union, bitwise
operators, loose/strict comparison, ordering, mixed switch, null coalescing, and
strict match are tested against PHP. Most other builtins keep concrete argument
checks. Union/nullable annotations, direct dynamic member access, resources and
references in native code, and full PHP coercion coverage remain outside the
compiled subset. PHP boundaries handle broader language/library behavior but
reject resources, references, callbacks, and native objects crossing the wire.
See [`MIXED.md`](MIXED.md).

**Generics (PXR extension).** `function identity<T>(T $value): T`
supports explicit `identity<int>(42)` and inferred `identity(42)` calls, with
concrete native specialization. Namespaces and compile-time modules work as for
ordinary functions. Generic syntax stays in `.php` files but is not valid PHP.
Function bodies are checked per instantiation; function signatures are always
checked. Generic classes/interfaces support `class Box<T>`, `new Box<int>(42)`,
typed fields/methods, inheritance, separate static state, and `instanceof Box<int>`.
Class templates are checked when instantiated; type arguments are explicit at
construction. Functions can infer arguments through generic class/interface
patterns. Methods can declare independent parameters with explicit/inferred
instance/static calls, interface/abstract contracts, and native virtual dispatch.
Generic overrides preserve signature shape up to parameter renaming. Generic
constructors/magic methods and class-parameter shadowing are rejected.
Bounds such as `<T: Printable & Named>` are checked nominally for explicit and
inferred instantiations. Concrete bodies can omit return annotations; ambiguous
cycles require an annotation (`E0284`). See [`INFERENCE.md`](INFERENCE.md).
Scalar widening does not convert `array<int>` storage to `array<float>`; such
calls fail at compile time. See [`GENERICS.md`](GENERICS.md).

**Traits.** Native traits compose typed properties and methods, including nested
trait use, namespace/module aliases, and consuming-class visibility/static
behavior. Class methods override traits; trait methods override inherited
methods. Conflicts fail explicitly. Adaptations, abstract trait requirements,
duplicate trait properties, and generic trait declarations are outside this
subset. See [`TRAITS.md`](TRAITS.md).

**Ownership (PXR extension).** `owned<T>` local/parameter/result bindings use
explicit `move($value)` transfer and optional `drop($value)`. `&T` / `&mut T`
parameters use explicit call-scoped borrow arguments. Shared borrow mutation,
escaping/stored borrows, copying owners, conflicting loans, and use after
move/drop fail with `E0290`. Mutable views apply to arrays/objects, not scalar
variable aliases; borrowed bindings cannot be rebound. Owned fields, nested
qualifiers, shared borrow method calls, and arbitrary PHP `&` references remain
rejected. See [`OWNERSHIP.md`](OWNERSHIP.md).

Existing array-copy/object-identity semantics remain intact. Native liveness and
tracing reclaim dead values and cycles. The PHP worker releases unreachable
transfer-table roots, so legacy destructor timing follows reachability and
collection safepoints rather than native PHP variable-scope timing. Shared child
objects keep their shared semantics; affine handles do not yet establish thread
transfer safety. Native destructors and deterministic resource cleanup are not
part of this memory-management milestone. Native `__destruct` declarations fail
explicitly with `E0290`; PHP-boundary destructors remain PHP behavior.

**Fail loud, never silent.** Features not yet implemented (such as
reference closures, exceptions and nullable annotations)
produce a compile error with a specific code — never a silently different program.

**Invokable objects.** `$callback($arg)` lowers to checked `__invoke` dispatch,
including interface callbacks, inheritance and receiver retention across awaited
arguments. The method uses the existing visibility/argument/ownership rules.
By-value, bound, static and PXR async closures/arrow functions are supported; see
[CALLABLES.md](CALLABLES.md). Callable strings/arrays, reference captures, static trait
closure scopes, lexical trait annotations and `callable` annotations remain unsupported. User-declared `__invoke` remains synchronous under the current magic-method policy.

## Runtime independence

Native PXR executables link only the system C library. Native-only programs,
including mixed operations, do not require PHP. Executing an explicit PHP
boundary starts a persistent PHP CLI worker; it does not embed Zend or a SAPI. See [`RUNTIME.md`](RUNTIME.md).
