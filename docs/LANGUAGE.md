# The PXR Language (milestone 0)

This is the precise description of the language the current compiler accepts.
It is written to match the implementation and the test suite — if this document
and the compiler disagree, that is a bug.

PXR is **statically typed**. There is no `mixed` yet; every value has exactly one
of the scalar types below, known at compile time.

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

The only implicit coercion is the lossless widening **`int` → `float`**. All other
mismatches are compile errors.

## A quick tour of the supported subset (verbatim from the grammar)

```
program   := topstmt*
topstmt   := fn | stmt
fn        := 'function' NAME '(' params? ')' (':' type)? block
params    := param (',' param)*
param     := type VARIABLE
type      := 'int' | 'float' | 'string' | 'bool' | 'void' | NAME
block     := '{' stmt* '}'

stmt      := 'echo' expr (',' expr)* ';'
           | 'print' expr ';'
           | 'return' expr? ';'
           | 'if' '(' expr ')' body ('elseif' '(' expr ')' body)* ('else' body)?
           | 'while' '(' expr ')' body
           | block
           | VARIABLE (':' type)? '=' expr ';'    // assignment (optionally typed)
           | expr ';'
           | ';'
body      := block | stmt

expr      := or
or        := and ('||' and)*
and       := equality ('&&' equality)*
equality  := relational (('==' | '!=' | '===' | '!==') relational)*
relational:= concat (('<' | '<=' | '>' | '>=') concat)*
concat    := additive ('.' additive)*
additive  := multiplicative (('+' | '-') multiplicative)*
multiplicative := unary (('*' | '/' | '%') unary)*
unary     := ('!' | '-' | '+') unary | primary
primary   := INT | FLOAT | STRING | 'true' | 'false' | 'null'
           | VARIABLE
           | NAME '(' (expr (',' expr)*)? ')'
           | '(' expr ')'
```

Operator precedence follows PHP 8 (in particular `.` binds looser than `+`/`-`,
and `!` binds tighter than both).

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

Unary `-` requires a numeric operand. Unary `!` yields `bool`.

## Builtin functions

| Builtin | Signature | Notes |
| --- | --- | --- |
| `strlen(string): int` | byte length | `E0234` if not a string |
| `intval(int\|float\|bool\|string): int` | convert to int | string parses leading numeric prefix |
| `floatval(int\|float\|bool\|string): float` | convert to float | |
| `strval(int\|float\|bool\|string): string` | convert to string | |
| `abs(int): int`, `abs(float): float` | absolute value | |

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
| `E0003` | Bitwise operator encountered (not implemented) |
| `E0004` | Unexpected character |
| `E0005` | Invalid numeric literal |
| `E0006` | Integer literal out of range |
| `E0007` | Unterminated string literal |
| `W0001` | Leading‑zero octal treated as decimal |
| `W0002` | String interpolation not implemented (kept literal) |
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
| `W0202` | `return` with a value at top level is ignored |
