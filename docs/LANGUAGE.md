# The PXR Language (milestones 0–1)

This is the precise description of the language the current compiler accepts.
It is written to match the implementation and the test suite — if this document
and the compiler disagree, that is a bug.

PXR is **statically typed**. There is no `mixed` yet; every value has exactly one
type known at compile time (scalars, or a homogeneous array).

## Source encoding

* Files use the `.php` extension. The extension is not special to the compiler
  (you can compile any path), but the convention is universal: compilation is
  chosen with the `pxr` tool, never by a new extension.
* A leading `#!` shebang line is ignored.
* An optional `<?php` (or `<?`) opening tag is skipped. A trailing `?>` ends
  tokenization; content after it is ignored with a warning.
* Comments: `//`, `#` to end of line, and `/* ... */`.

## Types

| Type | Meaning | Representation |
| --- | --- | --- |
| `int` | 64‑bit signed integer | `i64` |
| `float` | IEEE‑754 double | `double` |
| `string` | immutable byte/UTF‑8 string | `const char *` |
| `bool` | boolean | `i1` |
| `void` | no value (return types only) | — |
| `array<T>` | ordered map with values of type `T` (`int`/`string` keys) | opaque `ptr` |

The only implicit coercion is the lossless widening **`int` → `float`**. All other
mismatches are compile errors.

## A quick tour of the supported subset (verbatim from the grammar)

```
program   := topstmt*
topstmt   := fn | stmt
fn        := 'function' NAME '(' params? ')' (':' type)? block
params    := param (',' param)*
param     := type VARIABLE
type      := 'int' | 'float' | 'string' | 'bool' | 'void' | 'array' '<' type '>' | NAME
block     := '{' stmt* '}'

stmt      := 'echo' expr (',' expr)* ';'
           | 'print' expr ';'
           | 'return' expr? ';'
           | 'if' '(' expr ')' body ('elseif' '(' expr ')' body)* ('else' body)?
           | 'while' '(' expr ')' body
           | 'do' body 'while' '(' expr ')' ';'
           | 'for' '(' for_expr? ';' expr? ';' for_expr? ')' body
           | 'foreach' '(' expr 'as' VARIABLE ')' body
           | 'switch' '(' expr ')' '{' switch_case* '}'
           | 'break' INT? ';'
           | 'continue' INT? ';'
           | block
           | lvalue (':' type)? '=' expr ';'          // assignment (optionally typed)
           | lvalue ('+=' | '-=' | '*=' | '/=' | '%=' | '.=' | '&=' | '|=' | '^=' | '<<=' | '>>=' | '??=') expr ';'
           | lvalue ('++' | '--') ';'
           | expr ';'
           | ';'
body      := block | stmt

switch_case := ('case' expr ':') stmts | ('default' ':') stmts | ('case' expr ':') | ('default' ':')
for_expr  := lvalue '=' expr                     // also allows '+=', ++, -- ...

lvalue    := VARIABLE
           | lvalue '[' expr? ']'                // index / append

expr      := ternary
ternary   := coalesce ('?' expr? ':' expr)?      // right-associative
coalesce  := or ('??' or)*                       // right-associative
or        := and ('||' and)*
and       := bit_or ('&&' bit_or)*
bit_or    := bit_xor ('|' bit_xor)*
bit_xor   := bit_and ('^' bit_and)*
bit_and   := equality ('&' equality)*
equality  := relational (('==' | '!=' | '===' | '!==') relational)*
relational:= shifts (('<' | '<=' | '>' | '>=' | '<=>') shifts)*
shifts    := concat (('<<' | '>>') concat)*
concat    := additive ('.' additive)*
additive  := multiplicative (('+' | '-') multiplicative)*
multiplicative := unary (('*' | '/' | '%') unary)*
unary     := ('!' | '-' | '~') unary | primary
primary   := INT | FLOAT | STRING | 'true' | 'false' | 'null'
           | VARIABLE | primary '[' expr ']' | primary '[' ']'
           | '[' (array_item (',' array_item)*)? ','? ']'
           | 'array' '(' ... ')'
           | VARIABLE '(' (expr (',' expr)*)? ')' | NAME '(' (expr (',' expr)*)? ')'
           | 'match' '(' expr ')' '{' match_arm* '}'
           | '(' expr ')'
array_item := (expr '=>')? expr
match_arm  := (expr (',' expr)* | 'default') '=>' expr ','
```

Operator precedence follows PHP 8 (in particular `.` binds looser than `+`/`-`,
and `!` binds tighter than both). `??` is right‑associative and sits just below
`?:`; `?:` is right‑associative.

Only `+=`‑style assignment, `++`/`--` and plain `=` are *statements* — PXR
deliberately does **not** allow assignment expressions (so `$a = $b = 1` and
`f($x++)` are rejected).

## Function declarations

* Parameters are declared PHP‑style and **must carry a type**:

  ```php
  function add(int $a, int $b): int { return $a + $b; }
  ```

  This is valid PHP 7+ syntax, which is the point: typed modern PHP is already
  most of the way to a PXR signature.
* A return type is **required** (`: void` for procedures).
* Function names are **case‑insensitive** (like PHP). Variables are
  **case‑sensitive** (like PHP).
* Function declarations are hoisted, so functions may call functions declared
  later in the file, and recursion is supported.
* A non‑`void` function must `return` on every path; the analyzer rejects
  fall‑through.

The remaining top‑level statements (everything that is not a function
declaration) become the program entry point.

## Variables

* Assigning a new variable with `$x = expr;` infers its type from `expr`.
* An explicit PXR annotation may be given: `$x: int = 0;` (not valid PHP; a PXR
  extension).
* A variable's type is fixed after its first assignment. Reassigning a
  different, non‑widening type is an error.
* Using a variable before any assignment is an error (`E0220`).

## Expressions and operators

Arithmetic `+ - * /` require numeric operands; `%` requires two `int`s.
`/` always produces `float` (PHP semantics), even for two `int`s.

Concatenation `.` accepts `string`, and also `int`/`float`/`bool`, which are
converted to their string form.

Equality:

* `==` / `!=` — loose: numeric operands are compared numerically (with
  `int`→`float` widening); `string` with `string`; `bool` with `bool`.
* `===` / `!==` — strict: operands must have the **same** type.

Ordering `< <= > >=` — both numeric, or both `string` (lexicographic).

Logical `&& || !` — operands are used in boolean context. Conditions in `if` and
`while` likewise accept any *truthy* scalar: `bool`, `int` (`!= 0`), `float`
(`!= 0.0`, and `NaN` is truthy), `string` (non‑empty and not `"0"`). `&&` and
`||` short‑circuit, so the right‑hand side is not evaluated when the result is
already known.

Unary `-` requires a numeric operand. Unary `!` yields `bool`. Unary `~` requires
an `int` and yields the bitwise complement.

Additional operators:

* `<=>` (spaceship) — compares two numerics or two strings and yields `int`
  `-1`/`0`/`1`.
* `??` (null coalescing) — yields the left operand when it "is set". In PXR only
  an array element read can be unset; for any other expression `??` degrades to
  its left operand. The right operand is only evaluated when needed.
* `& | ^ << >> ~` — bitwise operators require `int` operands (PHP would coerce;
  PXR rejects at compile time). Shifts follow PHP's rules exactly: `>>` is an
  arithmetic (sign‑preserving) shift, a count of 64 or more yields `0` (or `-1`
  for `>>` of a negative value), and a negative count is a fatal
  `ArithmeticError`.

## Arrays

PXR arrays are **homogeneous, ordered maps** with PHP semantics: insertion order
is preserved, and keys are `int` or `string`. The element type is fixed at
compile time; the syntax uses PHP's `[...]` literal and `array(...)` form.

```php
$a = [10, 20, 30];        // array<int>, keys 0,1,2
$a[] = 40;                // append; key 3
$a[0] = 11;               // overwrite
$m = ["x" => 1, "y" => 2]; // array<int>, string keys
$grid = [[1, 2], [3, 4]];  // array<array<int>>
echo $a[0] . " " . count($a) . " " . $m["y"] . "\n";
```

Rules and limitations:

* A type annotation is written `array<T>` (a PXR extension; not valid PHP). A
  bare `array` type hint (valid PHP) is rejected — write the element type.
* An empty `[]` has an as‑yet‑unknown element type that is **refined** by the
  first `push`/`set`; reading an element of a still‑empty array is an error
  (`E0243`).
* Keys must be `int` or `string` (`E0250`). A `float` key is rejected.
* You cannot `echo`/`print` an array (`E0240`); index it or use a loop.
* Nested assignment `$a[0][1] = x` is not supported yet (`E0248`); assign the
  inner array to a variable first.
* `foreach` currently supports the **value‑only** form `foreach ($a as $v)`. The
  keyed form `foreach ($a as $k => $v)` is rejected (`E0242`); iterate with an
  explicit index and `count()` instead.
* Arrays have PHP **value semantics**: `$b = $a`, passing `$a` to a function,
  returning an array, and `foreach` all operate on a copy (realised as an eager
  deep copy), so mutating a copy never affects the original.
* Canonical decimal‑integer string keys **are** the integer key (`$a["1"]` is
  `$a[1]`), exactly as in PHP; `"01"`, `"1.5"`, `"+1"` and `" 1"` stay strings.

## Control flow

* `if` / `elseif` / `else` — unchanged from milestone 0.
* `while (cond) body`, `do body while (cond);`.
* `for (init; cond; update) body` — `init` and `update` are lists of assignment
  statements; `cond` may be omitted for an infinite loop. The `continue` target
  is the `update` step.
* `foreach ($arr as $v) body` — value‑only (see arrays above).
* `break` / `continue` with an optional level: `break 2;` exits two nested
  loops. A `switch` counts as one level, so `continue` inside a `switch` behaves
  like `break` (as in PHP).
* `switch (subject) { case v: ... default: ... }` — preserves PHP fall‑through
  (a `case` without `break` falls into the next body). Comparisons are loose
  (`==`), producing PHP's numeric/string coercions for the supported types.
* `match (subject) { v1, v2 => r, default => r }` — strict (`===`), evaluated
  top to bottom; the `default` arm matches only when no other arm matches
  (regardless of its position, as in PHP). PXR **requires** a `default` arm
  (`E0256`) because there is no runtime exception to throw when nothing matches.

Conditions in `if`/`while`/`for`/`do` accept any truthy scalar. `switch`/`match`
subjects are compared with loose/strict equality respectively.

## Builtin functions

| Builtin | Signature | Notes |
| --- | --- | --- |
| `strlen(string): int` | byte length | `E0234` if not a string |
| `intval(int\|float\|bool\|string): int` | convert to int | string parses leading numeric prefix |
| `floatval(int\|float\|bool\|string): float` | convert to float | |
| `strval(int\|float\|bool\|string): string` | convert to string | |
| `abs(int): int`, `abs(float): float` | absolute value | |
| `count(array<T>): int` | number of elements | `E0234` if not an array |
| `strtoupper(string): string` | ASCII bytes → upper | `é` stays `é`, like PHP |
| `strtolower(string): string` | ASCII bytes → lower | |
| `trim(string[, string]): string` | strip ends | optional byte mask; default `` " \t\n\r\0\x0B" ``; explicit `""` strips nothing |
| `ltrim(string[, string]): string` | strip left end | |
| `rtrim(string[, string]): string` | strip right end | |
| `substr(string, int[, int]): string` | substring | PHP 8 semantics: clamp, negative length, `""` never `false` |
| `str_repeat(string, int): string` | repeat | negative count → fatal `ValueError` |
| `str_replace(string, string, string): string` | replace | non-overlapping, no rescan; array form rejected (`E0234`) |
| `implode(string, array<string\|int\|float\|bool>): string` | join with glue | 1-arg form `implode(array)` uses `""`; elements `(string)`-coerced |
| `explode(string, string[, int]): string[]` | split into `array<string>` | `0` limit ≡ `1`; negative drops trailing pieces; empty separator fatal `ValueError` |
| `str_contains(string, string): bool` | substring test | case-sensitive; empty needle is always found |
| `str_starts_with(string, string): bool` | prefix test | empty prefix always true |
| `str_ends_with(string, string): bool` | suffix test | empty suffix always true |
| `strrev(string): string` | reverse | byte-wise, so multi-byte sequences are reordered like PHP |
| `ucfirst(string): string` | uppercase first byte | ASCII only, like PHP's locale-independent `toupper` |
| `lcfirst(string): string` | lowercase first byte | ASCII only |
| `ucwords(string): string` | uppercase each word | ASCII; default whitespace delimiters `" \t\n\r\f\v"` |
| `str_split(string[, int]): string[]` | split into byte chunks | default length `1`; empty string → `[]`; length `< 1` fatal `ValueError` |
| `array_values(array<T>): array<T>` | values with keys dropped | reindexed `0..n-1` |
| `array_reverse(array<T>[, bool]): array<T>` | reverse order | default renumbers integer keys, preserves string keys; `true` preserves all keys |
| `array_merge(array<T>, ...array<T>): array<T>` | concatenate | integer keys renumbered, later string keys overwrite; all arguments must share one element type (`E0253` otherwise) |
| `array_slice(array<T>, int[, int[, bool]]): array<T>` | contiguous slice | negative offset/length as PHP; string keys kept, integer keys renumbered unless the flag is `true` |
| `array_pad(array<T>, int, T): array<T>` | pad to `\|size\|` | prepends when negative; no-op returns the array unchanged; `int` value widens to a `float` element type |
| `array_fill(int, int, T): array<T>` | fill `count` copies | keys `start .. start+count-1`; negative count → fatal `ValueError` |
| `array_key_exists(int\|string, array<T>): bool` | key presence test | canonical integer strings unify with integer keys |
| `str_pad(string, int[, string[, int]]): string` | pad to length | pad type `0`/`1`/`2` = left/right/both (default `1`); empty pad or bad type → fatal `ValueError` |
| `substr_count(string, string[, int[, int]]): int` | non-overlapping count | argument checks in PHP's order (needle, offset, length) → fatal `ValueError` |

## Scoping

Functions have a single flat scope (like PHP); there is no block scoping and no
shadowing in milestone 0. A variable assigned inside an `if`/`while` body is
visible afterwards.

## Diagnostics reference

Diagnostics carry a stable code. Lexical codes are `E000x`/`W000x`, parser
errors are `E0100`, semantic errors are `E020x`–`E023x`, and semantic warnings
are `W020x`.

| Code | Meaning |
| --- | --- |
| `E0001` | Unterminated block comment |
| `E0002` | `$` not followed by a name |
| `E0003` | *(retired — bitwise operators are now supported)* |
| `E0004` | Unexpected character |
| `E0005` | Invalid numeric literal |
| `E0006` | Integer literal out of range |
| `E0007` | Unterminated string literal |
| `W0001` | Leading‑zero octal treated as decimal |
| `E0008` | Double‑quoted string interpolation not implemented |
| `E0100` | Syntax error (with the unexpected token described) |
| `E0201` | Duplicate function definition |
| `E0202` | Parameter needs an explicit type |
| `E0203` | Parameter cannot be `void` |
| `E0204` | Function needs an explicit return type |
| `E0205` | Unknown named type |
| `E0206` | Function must return on all paths |
| `E0207` | Nested function declarations unsupported |
| `E0208` | Condition is not a scalar |
| `E0209` | `null` is not supported yet |
| `E0210` | Variable redeclared with a different type |
| `E0211` | Variable cannot be `void` |
| `E0212` | Cannot assign value to variable |
| `E0213` | Function must return a value |
| `E0214` | `void` function must not return a value |
| `E0215` | Return type mismatch |
| `E0220` | Undefined variable |
| `E0221` | Unary operand type error |
| `E0222` | `%` requires two `int`s |
| `E0223` | `.` operand type error |
| `E0224` | Incomparable operands (`==`/`!=`) |
| `E0225` | `===`/`!==` requires identical types |
| `E0226` | Unorderable operands (`<` …) |
| `E0227` | Logical operator requires scalars |
| `E0228` | Operator cannot be applied to operands |
| `E0230` | Wrong number of arguments |
| `E0231` | Argument type mismatch |
| `E0232` | Call to undefined function |
| `E0233` | Builtin called with wrong arity |
| `E0234` | Builtin argument type error |
| `E0240` | Cannot `echo`/`print` an array |
| `E0241` | `for` condition has the wrong type |
| `E0242` | Keyed `foreach` not implemented |
| `E0243` | Array element type is still unknown (empty `[]`) |
| `E0244` | `foreach` over a non-array |
| `E0245` | `foreach` value variable type mismatch / cannot assign |
| `E0246` | `continue` outside a loop |
| `E0247` | `break` outside a loop or `switch` |
| `E0248` | Nested array assignment not supported |
| `E0249` | Cannot index a value of non‑array type |
| `E0250` | Array key is not `int` or `string` |
| `E0251` | Invalid array element type annotation |
| `E0252` | `++`/`--` applied to a non‑numeric target |
| `E0253` | Cannot combine branch/coalesce result types |
| `E0254` | `[]` append used outside an assignment target |
| `E0255` | `match` arm condition type differs from the subject |
| `E0256` | `match` requires a `default` arm |
| `E0257` | `**` (power) not implemented |
| `W0202` | `return` with a value at top level is ignored |
