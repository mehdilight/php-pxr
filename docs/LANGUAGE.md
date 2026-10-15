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
| `ClassName` | reference to class instance (`Ty::Named`) | opaque `ptr` |

The only implicit coercion is the lossless widening **`int` → `float`**. All other
mismatches are compile errors.

## A quick tour of the supported subset (verbatim from the grammar)

```
program   := topstmt*
topstmt   := class_decl | fn | stmt
class_decl:= 'class' NAME '{' member* '}'
member    := prop_decl | method_decl
prop_decl := visibility type VARIABLE ('=' expr)? ';'
method_decl := visibility 'function' NAME '(' params? ')' (':' type)? block
visibility:= 'public' | 'protected' | 'private'
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
           | postfix_expr '->' NAME              // property access

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
unary     := ('!' | '-' | '~') unary | postfix
postfix   := postfix '->' NAME '(' (expr (',' expr)*)? ')'  // method call
           | postfix '->' NAME                              // property access
           | postfix '[' expr ']' | postfix '[' ']'
           | primary
primary   := INT | FLOAT | STRING | INTERP_STRING | 'true' | 'false' | 'null'
           | VARIABLE
           | 'new' NAME ('(' (expr (',' expr)*)? ')')?
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

A double‑quoted string may interpolate a variable with `"$name"` or `"{$name}"`;
this is syntactic sugar for `"" . $name`, so the value is string‑converted like
any other `.` operand. A `$` that is not followed by a name start is an ordinary
`$` (`"$1"`, `"foo$"`, `"$$x"` behave as in PHP). PHP's richer interpolation
forms — `"$a[0]"`, `"$a[$k]"`, `"{$a[0]}"`, `"$obj->p"` and the deprecated
`"${name}"` — are not implemented and are rejected with `E0008`.

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
| `addslashes(string): string` | backslash-escape `` ' `` `"` `\` | a NUL byte becomes the two characters `\0` |
| `stripslashes(string): string` | undo `addslashes` | drops each backslash and emits the next byte (`\0` → NUL); matches PHP exactly, so `\n` → `n` |
| `bin2hex(string): string` | lowercase hex encoding | two hex digits per input byte |
| `base64_encode(string): string` | standard RFC 4648 base64 | `=`-padded |
| `urlencode(string): string` | form URL encoding | space → `+`, `~` percent-encoded; `A-Za-z0-9-_.` pass through |
| `rawurlencode(string): string` | RFC 3986 URL encoding | space → `%20`, `~` passes through |
| `urldecode(string): string` | form URL decoding | `+` → space, `%XX` decoded; invalid/incomplete `%` kept literally |
| `rawurldecode(string): string` | RFC 3986 URL decoding | like `urldecode` but `+` is left alone |
| `sprintf(string, (int\|float\|string\|bool)...): string` | formatted string creation | PHP conversion specifiers, modifiers, widths, precision, positional args |
| `printf(string, (int\|float\|string\|bool)...): int` | formatted string output | prints to stdout, returns length in bytes |

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
| `E0008` | Unsupported double‑quoted interpolation form (`$a[...]`, `{$expr}`, `$obj->p`, `${...}`) |
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
| `E0260` | Operand of `instanceof` is not an object |
| `E0261` | Unknown property |
| `E0262` | Unknown method |
| `E0265` | Cannot access private or protected member / narrowing visibility |
| `E0267` | Cannot re-assign `$this` |
| `E0268` | Cannot use `$this` outside object context / property type mismatch |
| `E0269` | Invalid abstract declaration, missing implementation, abstract/interface instantiation, or abstract parent call |
| `E0270` | Invalid class/interface inheritance edge, duplicate supertype, or non-public interface method |
| `E0271` | Static-member misuse, static/instance modifier mismatch, or unsupported static form |
| `E0273` | Invalid/unresolved compile-time module dependency or missing file context |
| `W0202` | `return` with a value at top level is ignored |

## Classes, Inheritance & Polymorphism

* **Class declaration**: `class Name [extends ParentName] { ... }`
* **Properties**: `public|protected|private Type $name [= default];`
* **Methods**: `public|protected|private function name(Type $arg): RetType { ... }`
* **Constructor**: `public|protected|private function __construct(...) { ... }`
  * If omitted, subclasses inherit parent constructors.
  * Static dispatch via `parent::__construct(...)` and `parent::method(...)`.
* **Polymorphism**:
  * Subclass instances are assignable to parent class variables, parameters, and return types.
  * Virtual method calls dispatch to the subclass implementation at runtime via synthesized jump tables (`pxr$disp$<root>$<method>`).
* **`instanceof` operator**:
  * `$obj instanceof ClassName` returns `bool`, traversing the parent hierarchy at runtime.
  * Left operand must be an object type (`E0260`).


* **Abstract classes**: `abstract class Name [extends ParentName] { ... }`.
  Abstract classes cannot be instantiated. Concrete subclasses must implement all
  inherited abstract methods, including abstract constructors.
* **Abstract methods**: `abstract public|protected function name(Type $arg): RetType;`.
  Visibility may also precede `abstract`; omitting it defaults to public.
  Abstract methods have no body and cannot be private. Implementations obey the
  existing signature and visibility rules. Calling an abstract method through
  `parent::` is rejected; object calls use dynamic dispatch.

* **Interfaces**: `interface Name [extends A, B] { public function f(Type $x): Ret; }`.
  Methods are implicitly abstract and must be public; visibility may be omitted.
  Bodies, explicit `abstract` modifiers, properties, and constants
  are not supported in this slice. Static method signatures are supported.
* **Implementation**: `class Name [extends Parent] implements A, B { ... }`.
  Concrete classes must implement all interface methods, including constructors;
  inherited public methods can satisfy a contract. Abstract classes can defer
  implementations. Parameter types are invariant; named return types may narrow
  to a subtype. Multiple inherited method contracts must all be compatible.
* Interface types can be used wherever named object types are accepted. Calls
  through interface types dispatch to the concrete object's method. `instanceof`
  includes interfaces inherited through classes and other interfaces.

## Static members

* Properties: `public static int $count = 0;`. Types can be inferred from defaults;
  static defaults must be scalar or array constant expressions in this slice.
* Methods: `public static function next(): int { ... }`. Static methods have no
  `$this`. Visibility, `static`, and `abstract` modifiers may appear in any order;
  duplicate modifiers are rejected. Constructors cannot be static.
* Access: `Counter::$count`, `Counter::$count++`, `Counter::next()`, `self::$count`,
  `self::next()`, `parent::$count`, and `parent::next()`.
* `self` resolves lexically to the class declaring the method. Inherited static
  properties share storage until a subclass redeclares that property.
* Static interface and abstract class method signatures require compatible
  implementations with the same static modifier. Qualified calls to abstract
  declarations are rejected.
* `static::` late binding, dynamic class names, function-local statics, object-style
  static calls, uninitialized static properties, and direct array element writes
  through properties are not supported yet. Use a local array copy and assign the
  whole property when updating an array-valued static property.

## Namespaces and imports

* `namespace App\Feature;` applies to the following declarations and code.
  `namespace App\Feature { ... }` scopes a braced block; `namespace { ... }`
  selects the global namespace. A file cannot mix braced and unbraced forms.
* `use Vendor\Thing as Alias;` imports a class/interface or namespace prefix;
  `use function Vendor\helper as localHelper;` imports a function. Aliases are
  local to the file and namespace block; comma-separated imports are supported.
  Grouped imports and `use const` are not implemented.
* `Thing` resolves in the current namespace or through its import;
  `\Vendor\Thing` is absolute, and `namespace\Thing` is namespace-relative.
* Unqualified function calls try the namespaced function and then the global
  function or builtin. Explicitly qualified calls have no global fallback.
* Namespaces and imports must be top-level. Namespace declarations precede code.

## Compile-time modules

`require_once __DIR__ . "/Library.php";` loads declarations at compile time.
Literal `require "Library.php";` is also supported. Relative paths resolve from
that file's directory. Dependencies must precede executable top-level statements;
loaded files may contain only declarations and dependencies. Once dependencies
are deduplicated by canonical path. Non-once cycles and dynamic paths are errors.
See [`MODULES.md`](MODULES.md) for examples and the distinction from runtime PHP
inclusion. `tokens` and `ast` inspect only the requested file; semantic/build
commands resolve its full dependency graph.
