# Milestones

PXR is built incrementally. A milestone is only marked **done** when it works
end‑to‑end and is covered by tests; otherwise it is **planned** (or **in
progress**). This file is the single source of truth for what exists.

Legend: ✅ done · 🚧 in progress · ⛔ planned

---

## ✅ M0 — Minimal but fully functional end‑to‑end compiler

**Goal.** Parse real `.php` files, validate types, generate native machine code,
link an executable, and report meaningful errors.

**Delivered.**

* `pxr-lexer` — PHP‑grammar lexer over bytes with precise spans; comments, tags,
  shebang, numeric and string literals, case‑insensitive keywords.
* `pxr-ast` + `pxr-parser` — recursive‑descent / precedence‑climbing parser with
  statement‑level error recovery.
* `pxr-sema` — scope and variable resolution, type inference, and all static
  checks (`E020x`–`E023x`).
* `pxr-ir` — typed three‑address IR with an SSA‑style value model and explicit
  casts.
* `pxr-opt` — constant folding, constant‑branch folding, dead‑code elimination,
  CFG cleanup, with statistics.
* `pxr-codegen` — typed AST → IR lowering (coercions, short‑circuit `&&`/`||`,
  builtin expansion) and IR → textual LLVM IR.
* `pxr-driver` — pipeline orchestration, embedded + cached runtime, `clang`
  invocation, linking.
* `pxr` — CLI: `build`, `run`, `check`, `emit-ir`, `emit-llvm`, `tokens`, `ast`.
* `runtime/pxr_rt.c` — standalone runtime (printing, strings, conversions,
  truthiness, arena).
* Language subset: scalars `int/float/string/bool/void`, typed functions,
  `echo`/`print`, `if`/`elseif`/`else`, `while`, `return`, all core operators,
  builtins `strlen`/`intval`/`floatval`/`strval`/`abs`.
* Tests: 29 unit tests across the crates + 11 end‑to‑end CLI tests that compile
  and run native binaries; `examples/compat.php` output is asserted byte‑identical
  to PHP.
* Benchmarks: naive `fib(30)` ≈ 12× and a 5M‑iteration loop ≈ 33× faster than the
  PHP interpreter on the reference machine.

**Acceptance criteria — met.**

1. `pxr build examples/fib.php -o fib && ./fib` produces a native executable that
   prints the Fibonacci sequence. ✅
2. The executable links only `libSystem` and runs without PHP. ✅
3. `pxr check` rejects undefined variables, type mismatches and missing returns
   with rendered, caret‑annotated diagnostics. ✅
4. `pxr run examples/compat.php` matches `php examples/compat.php` byte for byte. ✅
5. `cargo test` passes. ✅

**Known limitations** (carried into later milestones): no arrays/classes/null,
integer overflow wraps, strings leak per‑operation (arena, no free), no
concurrency, no C interop, no generics, no ownership.

---

## ✅ M1 — Language breadth I (control flow & data)

* Arrays (indexed and associative) with `[]` literals, indexing, assignment.
* `for`, `foreach`, `do…while`, `break`, `continue`, `switch`, `match`.
* Ternary `? :`, null coalescing `??`, spaceship `<=>`.
* Compound assignment `+= -= *= /= .=` and `++`/`--`.
* Bitwise operators `& | ^ ~ << >>`.

**Delivered.**

* `pxr-lexer` — new keywords (`for`, `foreach`, `as`, `do`, `break`, `continue`,
  `switch`, `case`, `default`, `match`, `array`), operators `| ^ ~ & << >> ?? <=>
  =>`, and the compound assignments `&= |= ^= <<= >>= ??=`.
* `pxr-ast` + `pxr-parser` — new statements (`for`, `do…while`, `foreach`,
  `switch`, `break`/`continue` with levels), `LValue` targets (`$a`, `$a[k]`,
  `$a[]`), array literals/`array()`, `match`, ternary, and a PHP‑8‑accurate
  precedence table (`??` below `?:`, `.` above shifts, `<=>` among relationals).
* `pxr-types` — `Ty::Array(Box<Ty>)`; `array<null>` (an empty literal) is
  assignable to any array and is refined on first use.
* `pxr-sema` — array typing and element refinement, index/push targets,
  `count`, all control‑flow analyses (`break`/`continue` level resolution,
  `switch`/`match` typing, strict `match` with a required `default`), and the
  `E024x`/`E025x` checks. Loose `switch`/`===` `match` semantics.
* `pxr-ir` — `BinOp::{Cmp,BitAnd,BitOr,BitXor,Shl,Shr}`, `UnOp::BitNot`, and
  `Inst::BitCast` (used to pack/unpack array payloads).
* `pxr-codegen` — lowers all new statements/expressions; `&&`/`||`/`??`/`?:`
  become real control flow; loops/`switch` maintain `break`/`continue` label
  stacks; array operations lower to the `pxr_array_*` runtime calls with the
  tag+payload ABI. Backend emits the new integer ops plus a 3‑way `<=>`.
* `runtime/pxr_rt.c` — the ordered‑map array API and a page‑list arena (so
  growing it never invalidates earlier pointers).
* Tests: +8 end‑to‑end CLI tests (`arrays_and_foreach`, `control_flow_breadth`,
  `quicksort_matches_php`, `array_misuse_is_rejected`,
  `array_element_type_refinement`, `emit_llvm_declares_array_runtime`, plus
  `compat_example_matches_php` and `quicksort_example_runs`) — 19 CLI tests total.
* Examples: `examples/quicksort.php`; `examples/compat.php` extended with the
  milestone‑1 surface and still asserted byte‑identical to PHP.

**Acceptance criteria — met.**

1. `pxr run examples/quicksort.php` sorts the array and prints
   `1 2 3 4 5 6 7 8 9 10`, identical to `php examples/quicksort.php`. ✅
2. `pxr run examples/compat.php` remains byte‑identical to PHP, now covering
   arrays, `for`/`while`/`do…while`/`foreach`, `switch`, `match`, `<=>`, ternary,
   compound assignment and bitwise operators. ✅
3. Misusing arrays (echoing one, keyed `foreach`, nested assignment, bad keys)
   fails at compile time with a specific code. ✅
4. `cargo test` passes. ✅

**Known limitations** (carried into later milestones): `foreach` is value‑only;
no `echo` of arrays; no nested assignment; no `null`; integer overflow still
wraps (PHP promotes to `float`); a variable cannot change type; strings/arrays
leak per‑operation (arena).

Arrays already have PHP **value semantics** (assignment, arguments, returns and
`foreach` copy) and canonical integer string keys unify with integer keys.

Later fidelity fixes: shifts follow PHP (`<<`/`>>` with count ≥ 64 → `0`/`-1`,
negative count → fatal `ArithmeticError`), loose string comparisons are
numeric-aware (`"10" < "9"` = false, `"1" == "01"` = true, `===` stays byte
equality, `switch` follows), floats stringify exactly like PHP (`zend_gcvt`
precision 14: `1e15` → `"1.0E+15"`, `1e-5` → `"1.0E-5"`, `-0.0` → `"-0"`),
`/` and `%` by zero raise PHP's fatal `DivisionByZeroError`, and double-quoted
interpolation was (at that point) rejected at compile time as `E0008`, pending
the M2.interpolation slice. Strings became **owned
byte strings** (`pxr_str { len, data[] }`, still a single `ptr` in the ABI), so
embedded NULs are first-class: `strlen("a\0b")` is `3`, equality/ordering and
concatenation are byte-exact — matching PHP.

## ✅ M2 — Strings & standard library

* Owned string type (length + data, embedded NUL safe) — **done** in the M1.x
  fidelity round; simple double‑quoted interpolation (`"$name"` / `"{$name}"`)
  also done (M2.interpolation slice); the richer PHP forms remain rejected.
* A real standard library (sorted by module) reachable via `use`, compiled from
  PXR sources or intrinsic.
* `sprintf`, `explode`/`implode`, `str_replace`, `substr`, `str_*`, `array_*`, encoding/escaping helpers.

**Delivered (M2.strings slice).**

* `strtoupper`, `strtolower` — ASCII byte mapping, `"héllo"` → `"héllo"` like PHP.
* `trim`/`ltrim`/`rtrim` — PHP default mask `` " \t\n\r\0\x0B" ``, byte masks, an
  explicit empty mask strips nothing (`mask == NULL` means "default").
* `substr` — PHP 8 exact: offsets clamp, negative length cuts from the end,
  out-of-range → `""` (never `false`).
* `str_repeat` — negative count → PHP's fatal `ValueError` (exit 255).
* `str_replace` — string form only, PHP's non-overlapping left-to-right scan.
* `implode` — `(array)` / `(separator, array)`, scalar elements `(string)`-coerced
  via the same `zend_gcvt` float path; arrays of arrays rejected at compile time.
* `explode` — `array<string>`, positive/`0`/negative limits exactly as PHP,
  empty separator → fatal `ValueError`; default limit `PHP_INT_MAX`.
* Infrastructure: `Callee::Builtin` now carries `Vec<Ty>` argument types; the 1‑arg
  builtin machinery became a general multi‑arg/optional‑arg lowering (`substr`
  length, `explode` limit, `trim` mask, `implode` glue all default in codegen);
  `Literal::Null` added for the "no charlist" pointer.
* All operations are length‑aware, so embedded NULs flow through everywhere
  (`substr("a\0b", 1, 1)` → `"\0"`).
* Tests: +4 CLI tests (`string_builtins_match_php`, `explode_implode_match_php`,
  `str_repeat_negative_is_fatal`, `explode_empty_separator_is_fatal`) — 31 CLI
  tests total; `examples/compat.php` extended and still asserted byte‑identical
  to PHP against PHP 8.4.

**Delivered (M2.predicates & array transforms slice).**

* `str_contains`, `str_starts_with`, `str_ends_with` — case-sensitive,
  NUL-safe, empty needle/prefix/suffix always true. Return `bool` (narrowed from
  the runtime's `i64` via `IntToBool`).
* `strrev` (byte-wise), `ucfirst`/`lcfirst` (ASCII first byte only), `ucwords`
  (default whitespace delimiters) — all match PHP's locale-independent helpers.
* `str_split` — `array<string>` of at-most-`length` byte chunks (default `1`),
  `""` → `[]` (PHP 8.2+), `length < 1` → fatal `ValueError` (exit 255).
* `array_values`, `array_reverse` — value→array transforms; both deep-copy nested
  arrays (value semantics). `array_reverse` renumbers integer keys and keeps
  string keys by default (optionally keeps all keys).
* **Bug fix (byte-string literals).** `\xHH` escapes for `HH ≥ 0x80` were passing
  through `String::from_utf8_lossy`, turning every high byte into a UTF‑8
  replacement character. String-literal payloads are now raw bytes
  (`Vec<u8>`) end to end (lexer → AST → sema → IR → LLVM pool), so `"\x80"` really
  is a one-byte string, as the docs already claimed. Guarded by a lexer unit test
  and the `string_literal_high_bytes_are_raw` CLI test.
* Tests: +5 CLI tests (predicates/case, `str_split`, array transforms,
  `str_split` fatal, raw bytes) and +1 lexer test — 36 CLI tests total;
  `examples/compat.php` extended (still byte‑identical to PHP).

**Delivered (M2.array builtins & `str_pad`/`substr_count` slice).**

* `array_merge` — variadic; integer keys are renumbered in order, a later string
  key overwrites an earlier one in place. Folds into a chain of binary runtime
  merges starting from an empty array, so even a single argument is reindexed.
  All arguments must share one element type (`[]`/`null` unifies with anything);
  a mixed `array<int>`/`array<float>`/`array<string>` merge is a loud `E0253`
  error (PXR arrays are homogeneous — PHP's mixed arrays are simply not a PXR
  value).
* `array_slice($a, $offset[, $length[, $preserve_keys]])` — exact PHP offset and
  length arithmetic (negative offsets count from the end, a negative length
  stops that many before the end, out-of-range clamps). String keys are always
  kept; integer keys are renumbered unless `$preserve_keys`.
* `array_pad($a, $size, $value)` — pad to `|$size|`; prepends when negative,
  appends when positive. With no padding needed PHP returns the array unchanged
  (keys preserved); otherwise integer keys are renumbered. An `int` value widens
  to a `float` element type.
* `array_fill($start, $count, $value)` — `$count` copies under integer keys
  `$start .. $start+$count-1` (negative start keys are preserved). `count < 0` is
  a fatal `ValueError`.
* `array_key_exists($key, $array)` — `int`/`string` keys only (a float/bool key is
  a loud type error); reuses the runtime key lookup behind `??`, so canonical
  integer strings (`"1"`) match integer keys.
* `str_pad($s, $length[, $pad[, $type]])` — `0`/`1`/`2` = left/right/both
  (`STR_PAD_*`; PXR has no constants yet, so pass the integer, default `1`).
  Each side cycles the pad string from its start. Empty pad string or an invalid
  pad type is a fatal `ValueError`.
* `substr_count($haystack, $needle[, $offset[, $length]])` — non-overlapping
  occurrence count; reproduces PHP's exact argument checks **and their order**
  (empty needle first, then offset, then length) and their fatal messages.
* All the new array builtins deep-copy nested arrays (PHP value semantics). The
  `Callee::Builtin` lowering now receives the resolved return type, so
  `array_merge`'s unified element type and `array_pad`'s element type are exact.
* Tests: +7 CLI tests (merge, slice/pad/fill/key-exists, `str_pad`/`substr_count`,
  three fatal cases, mixed-element rejection) — 43 CLI tests total;
  `examples/compat.php` extended (still byte‑identical to PHP 8.4).

**Delivered (M2.interpolation slice).**

* Double‑quoted interpolation of the simple forms `"$name"` and `"{$name}"`. The
  lexer gains a `TokenKind::InterpStr(Vec<StrPart>)` token (`Lit` runs + `Var`
  references); the parser desugars it into a left‑associative `"" . …`
  concatenation, so no sema/IR/codegen change was needed and the value is
  string‑converted exactly as PHP's echo/`.` conversion is.
* PHP‑exact literal‑`$` rule: `$` not followed by a name start stays a literal
  `$`, so `"$"`, `"$1"`, `"$ "`, `"foo$"` and `"$$x"` (→ `"$"` + `$x`) all match
  byte‑for‑byte; `\$` is an escaped dollar (already handled). A double‑quoted
  string with no actual variable folds back to a plain byte‑string token, so all
  existing string paths are untouched.
* The forms PXR cannot yet expand are **loud `E0008` errors** rather than being
  silently mis‑lexed: simple array access `"$a[0]"` / `"$a[$k]"`, property access
  `"$obj->p"`, complex `"{$expr}"` (only `"{$name}"` is accepted) and the
  deprecated `"${name}"`. Interpolating a value with no string conversion (an
  array) is a loud `E0223` type error, never PHP's `"Array"`‑plus‑warning.
* Tests: +2 CLI tests (`string_interpolation_matches_php`,
  `array_interpolation_is_a_loud_type_error`) and +2 lexer unit tests — 45 CLI
  tests total; `examples/compat.php` extended (still byte‑identical to PHP 8.4);
  the old `E0008`-on-any-`$` behaviour and its test were retired.

**Delivered (M2.escaping & encoding slice).**

* Eight `(string) -> string` helpers, all byte-exact vs PHP 8.4 over inputs
  spanning all 256 byte values (including high bytes and NULs), and all verified
  to round-trip identically:
  * `addslashes` / `stripslashes` — PHP's exact (and slightly surprising)
    semantics: `addslashes` escapes `'`, `"`, `\` and turns NUL into the two
    characters `\0`; `stripslashes` drops each backslash and emits the next byte,
    mapping `\0` back to NUL, so `\n` → `n` and a trailing backslash is dropped.
  * `bin2hex` — lowercase, two hex digits per byte.
  * `base64_encode` — standard RFC 4648 with `=` padding.
  * `urlencode` / `rawurlencode` — PHP's byte tables exactly (`urlencode`:
    space → `+`, `~` encoded; `rawurlencode`: space → `%20`, `~` kept).
  * `urldecode` / `rawurldecode` — `%XX` decoding, `+` → space for the former
    only, invalid/incomplete `%` sequences kept literally, as in PHP.
* All eight join the existing one-argument `(string) -> string` arm in sema, and
  share a small runtime helper (`pxr_url_encode`/`pxr_url_decode` with flags).
* Tests: +2 CLI tests (`escaping_and_encoding_builtins_match_php`, which
  cross-checks against the interpreter, and `encoding_builtin_type_errors_are_loud`)
  plus a reusable `assert_matches_php` helper — 47 CLI tests total;
  `examples/compat.php` extended (still byte‑identical to PHP 8.4).

**Delivered (M2.formatted printing slice).**

* `sprintf` and `printf` — formatted string creation and printing, reproducing
  PHP's formatting engine:
  * Specifiers: `%s` (string), `%d`/`%i` (signed int), `%u` (unsigned int),
    `%f`/`%F` (float decimal), `%e`/`%E` (exponential), `%g`/`%G`/`%h`/`%H`
    (shortest float), `%x`/`%X` (hexadecimal), `%o` (octal), `%b` (binary),
    `%c` (single char from ASCII byte), literal `%%`.
  * Modifiers: positional arguments (`%2$s %1$s`), sign prefix (`%+d`), padding
    (spaces, zeros with sign-aware placement, or custom padding character
    `%'*10s`), left-alignment (`%-10s`), width, and precision (`%.2f`, `%.3s`).
  * Argument coercion: scalar values (`int`, `float`, `string`, `bool`) coerce
    matching PHP's rules.
  * Runtime errors: too few arguments raises fatal `ArgumentCountError` (exit
    255); unknown format specifiers or missing specifier at end of string raise
    fatal `ValueError` (exit 255).
  * `printf` outputs to stdout via `fwrite` and returns the number of bytes written.
* Tests: +4 CLI tests (`sprintf_and_printf_match_php`, `sprintf_missing_args_is_fatal`,
  `sprintf_unknown_specifier_is_fatal`, `sprintf_type_errors_are_loud`) — 51 CLI
  tests total; `examples/compat.php` extended (still byte‑identical to PHP 8.4).

**Acceptance.** A string‑heavy program (e.g. word count) compiles and matches PHP —
the word‑count block at the end of `examples/compat.php` does exactly this. ✅

## ✅ M3 — User types & modules

* `class`, properties, methods, constructors, visibility (`public`, `protected`, `private`), `$this`, method chaining, instantiation (`new`).
* Single inheritance (`extends`), method overriding, `parent::`, `instanceof`.
* Interfaces, abstract classes, static properties and methods.
* `namespace`/`use`, a compile‑time module/include resolver.

**Delivered (M3.1: classes, properties, methods, constructors & visibility slice).**

* `class` declarations with typed properties, optional default values, methods,
  and explicit visibility modifiers (`public`, `protected`, `private`).
* Instantiation with `new ClassName(...)` and `new ClassName`, invoking
  `__construct` when defined, with arity and argument type validation.
* Property access (`$obj->prop`), property mutation (`$obj->prop = val`,
  `$obj->prop += val`, etc.), and method calls (`$obj->method(...)`) with full
  chaining support (`$a->b()->c`).
* Implicit `$this` binding within methods with static type `Ty::Named(class_name)`.
  Reassigning `$this` is rejected with `E0267`, and accessing `$this` outside an
  object context is rejected with `E0268`.
* Strict visibility enforcement at compile time: private properties and methods
  are inaccessible outside the declaring class (`E0265`).
* Object representation in runtime:
  `pxr_object { const pxr_str *class_name; size_t num_props; int64_t *tags; int64_t *bits; }`
  allocated in the bump arena, with $O(1)$ statically-indexed property slots
  supporting scalars, strings, arrays, and nested object references via bitcast packing.
  Null dereference causes PHP-faithful fatal error.
* Method dispatch is lowered directly to `TExpr::Call` with mangled link-time
  names (`pxr$m$<class>$<method>`) and `$this` prepended as the first parameter.
* Diagnostics added: unknown class (`E0205`), unknown property (`E0261`),
  unknown method (`E0262`), private member access violation (`E0265`),
  reassigning `$this` (`E0267`), using `$this` outside object context (`E0268`),
  and constructor arity mismatch (`E0207`).
* Tests: +4 CLI integration tests (`classes_and_objects_match_php`,
  `class_visibility_private_rejected`, `class_assign_this_rejected`,
  `class_type_errors_are_loud`) and unit tests — 55 CLI tests total;
  `examples/objects.php` added and verified byte-for-byte identical to PHP 8.4.

**Delivered (M3.2: single inheritance, method overriding, parent::, and instanceof slice).**

* Single inheritance with `class Child extends Parent`: inherits public and protected properties and methods; parent properties retain identical slot indices in child.
* Topological class sort and hierarchy validation: rejects self-inheritance (`E0201`), circular inheritance (`E0205`), and extending unknown classes (`E0205`).
* Method overriding compatibility rules:
  * Visibility cannot be narrowed from public to protected/private, or from protected to private (`E0265`).
  * Parameter count (`E0207`), parameter types (`E0208`), and return types (`E0208`) must match the inherited method.
* Constructor inheritance & `parent::`:
  * Child inherits parent `__construct` if none declared.
  * Child constructors and methods can invoke `parent::__construct(...)` or `parent::method(...)` (static dispatch to parent).
* Polymorphic dynamic method dispatch:
  * Synthesizes jump-table dispatchers (`pxr$disp$<root>$<method>`) when overrides exist in a class hierarchy, inspecting runtime `class_id` with zero overhead for non-overridden methods.
* `instanceof`:
  * Evaluates `$obj instanceof ClassName` as `bool`, walking the runtime parent hierarchy table; non-object operands rejected at compile time with `E0260`.
* Runtime updates in `pxr_rt.c`:
  * `pxr_object` stores `int64_t class_id`.
  * `pxr_class_register` populates runtime parent table at startup in `main`.
  * `pxr_object_get_class_id` and `pxr_instanceof`.
* Tests: +3 CLI integration tests (`inheritance_and_polymorphism_match_php`,
  `instanceof_hierarchy_matches_php`, `inheritance_diagnostics_rejected`) — 58 CLI tests total;
  `examples/inheritance.php` added and verified 100% byte-for-byte identical to PHP 8.4.

**Delivered (M3.3: abstract classes & methods slice).**

* `abstract class`, abstract method signatures ending in `;`, and both
  `abstract public` and `public abstract` modifier orders.
* Concrete descendants must implement all inherited abstract methods, including
  obligations carried through intermediate abstract classes. Implementations
  follow the existing signature and visibility checks; abstract constructors
  also enforce signature compatibility.
* Calls through abstract parent types and `$this` in inherited concrete methods
  dispatch to the concrete implementation. Abstract declarations remain linkable
  even when a hierarchy has no concrete descendants; unreachable fallbacks abort.
* Compile-time `E0269` errors for instantiating abstract classes, missing
  implementations, private abstract methods, replacing inherited concrete methods
  with abstract declarations, and calling abstract methods via `parent::`.
  Abstract methods with bodies and abstract properties are syntax errors (`E0100`).
* Four new CLI tests cover native execution, PHP output compatibility, unused
  hierarchies, constructors, modifier order, and invalid declarations — 62 CLI
  tests total. Example: `examples/abstract.php`.

**Delivered (M3.4: interfaces & implementation contracts slice).**

* `interface` declarations with public typed method signatures, multiple interface
  inheritance (`extends A, B`), and class declarations with `implements A, B`.
* Interface names share the class type namespace. Interface types work in
  parameters, return annotations, and other named-type positions. Implementations
  and inherited implementations must satisfy every contract, including constructor
  signatures; abstract classes can defer implementation to concrete descendants.
* Calls through interface types dispatch to concrete implementations, including
  inherited interface methods and overrides in implementing subclasses.
* `instanceof` recognizes directly implemented interfaces, extended interfaces,
  and interfaces inherited through parent classes. Runtime registration stores
  the complete supertype relation in arena-allocated hash buckets.
* Invalid graph edges, duplicate supertypes, unknown interfaces, cycles, method
  bodies/properties in interfaces, incompatible signatures, visibility violations,
  and interface instantiation produce compile-time errors. Interface method
  parameters are invariant in this initial subset; named return types may narrow.
  Numeric widening remains an expression coercion and is rejected in overrides.
* Four new CLI tests cover PHP output comparisons, interface dispatch, multiple
  inheritance, constructors, covariance, unused interfaces, and rejection cases —
  66 CLI tests total. Example: `examples/interfaces.php`.

**Delivered (M3.5: static properties & methods slice).**

* `static` properties and methods with public/protected/private visibility and
  modifier order independence; static interface and abstract method contracts.
* `Class::$property`, `Class::method(...)`, `self::`, and `parent::` access with
  member resolution, visibility, argument counts/types, and assignment checks.
  Instance methods retain `$this` for compatible class-qualified calls; static
  methods have no implicit receiver and reject `$this`.
* Arena-backed per-declaring-class storage initializes static defaults once at
  program startup. Inherited declarations share storage; subclass redeclarations
  have separate storage. Scalars and homogeneous arrays preserve existing value
  semantics. Compound assignments and increment/decrement use the property path.
* Static/instance modifier changes in overrides and interface implementations,
  static constructors, abstract qualified calls, and instance calls without a
  receiver are rejected at compile time. `E0271` covers static-member misuse and
  unsupported static forms.
* Initial limits: static properties require constant scalar/array defaults;
  uninitialized typed static properties, `static::` late binding, dynamic class
  names, object-style static calls, and static array element assignment remain
  unsupported and produce diagnostics.
* Four new CLI tests cover PHP output comparisons, inherited/redeclared storage,
  lexical `self`, `parent::`, interfaces, abstract methods, array copying,
  initialization, and invalid programs — 70 CLI tests total.
  Example: `examples/static.php`.

**Delivered (M3.6: namespaces, imports & compile-time modules slice).**

* Braced/unbraced `namespace` declarations, global namespace blocks, qualified
  and fully qualified names, and `namespace\Name` relative references.
* Class/namespace aliases (`use A\B as C`) and function imports
  (`use function A\f as g`), with file/block-local import tables and duplicate
  alias/declaration checks. Namespaced unqualified function calls try the local
  function first, then global functions/builtins; class names never fall back.
* Literal top-level `require`/`require_once` dependencies, including
  `__DIR__ . "/path.php"`, resolved relative to the containing file. Canonical
  paths deduplicate once dependencies, including safe require_once cycles;
  non-once cycles, missing files, dynamic paths, late/conditional dependencies,
  and dependencies with executable top-level statements are rejected.
* Dependencies are parsed independently and checked as one typed program.
  Diagnostics retain the originating file and source excerpt. `check`, `build`,
  `run`, `emit-ir`, and `emit-llvm` all use the same resolver; source-only APIs
  reject dependencies without a file context.
* Qualified LLVM symbol names use namespace-safe mangling. No runtime PHP or
  module loader is required by the compiled program.
* Five new CLI tests cover PHP output comparisons, namespace/import resolution,
  shared dependency graphs, once cycles, command consistency, included-file
  diagnostics, and invalid forms — 75 CLI tests total.
  Examples: `examples/namespaces.php`, `examples/modules.php`.

**Acceptance — met.** Namespaced code split across declaration-only PHP files
compiles to a native executable, matches PHP output, and reports errors against
the correct source file. Existing programs and all workspace tests pass. ✅

**Known limits.** No grouped or constant imports, autoloading, dynamic include
paths, dependency side effects, runtime `include`, or untyped PHP interop. Static
member limits from M3.5 remain. See [`MODULES.md`](MODULES.md).

**Next:** M4 — gradual migration (`mixed`, narrowing, and dynamic PHP interop).

## ✅ M4 — Gradual migration

* An opt‑in dynamic `mixed` type with runtime tags, and `is_*` narrowing.
* `include`/`require` for unmigrated PHP files (interop layer).
* A migration report: which parts of a project are statically typed vs dynamic.

**Acceptance.** A real (small) PHP project runs compiled, with the untyped parts
using `mixed`.

**Delivered (M4.1: tagged scalar mixed values slice).**

* Opt-in `mixed` annotations on parameters, returns, local variables, and
  properties. `int`, `float`, `bool`, and `string` values are automatically boxed
  at mixed boundaries; inferred concrete variables retain their existing types.
* Arena-allocated immutable `{ tag, bits }` scalar boxes, represented as a `ptr`
  in LLVM. Assignments, arguments, returns, constructors, property defaults, and
  compound concatenation preserve the destination representation.
* `is_int`, `is_float`, `is_bool`, `is_string`, `is_scalar`, `is_array`,
  `is_object`, and `is_null` predicates. Checks evaluate their arguments even
  when the result is known statically. Predicates do not yet narrow types.
* Explicit `intval`/`floatval`/`strval`/`boolval` conversions, printing,
  concatenation/interpolation, truthiness, strict `===`/`!==`, `match`, and
  branches combining mixed expressions with concrete scalar results.
* Homogeneous `array<mixed>` storage for explicitly boxed scalar elements, with
  array copying, append/index assignment, and existing array transforms. Whole
  arrays are not implicitly reboxed from `array<T>` to `array<mixed>`.
* Unsupported arithmetic, ordering, and loose comparisons on mixed values are
  loud `E0274` errors. M4.1 initially excluded array, object, and null boxing;
  M4.3 below adds null.
  Typed builtins continue to require concrete arguments except the conversions
  and predicates listed above.
* Five new CLI tests cover PHP output comparisons, scalar tags and conversions,
  strict comparisons, control flow, constructors/properties, opt-in reassignment,
  mixed array storage, and invalid programs — 80 CLI tests total.
  Example: `examples/mixed.php`. Details: [`MIXED.md`](MIXED.md).

**Delivered (M4.2: guard-based scalar narrowing slice).**

* Local mixed variables and parameters narrow after `is_int`, `is_float`,
  `is_bool`, and `is_string` guards, including negation, `elseif`/`else`, early
  returns, short-circuit `&&`/`||`, ternaries, boolean predicate comparisons,
  and successful strict comparisons with concrete scalar values.
* Reachable branch joins merge scalar possibility sets; returning paths do not
  erase facts needed by the surviving path. Exhausted scalar alternatives can
  identify the remaining type. User functions named like predicates do not
  create builtin type facts.
* Assignments invalidate previous facts. Loop analysis invalidates potential
  loop-carried writes before typing repeated code; `while`/`for` guards narrow
  bodies, and `for` updates retain guard facts only for values not rebound by
  the body. Fallthrough switch cases and reused foreach variables cannot retain
  stale refinements.
* Guarded compound numeric assignments and increment/decrement read the proven
  scalar type and store a newly boxed result. Storage remains mixed even when a
  read is narrowed. Foreach also boxes concrete elements when its destination
  variable was declared mixed.
* Four CLI tests cover PHP output comparisons, guard forms, mutation, loop
  iterations, branch joins, and rejection of unsound stale facts — 84 CLI tests
  total. Example: `examples/narrowing.php`.

**Delivered (M4.3: boxed null values and lazy coalescing).**

* Null literals use mixed storage, distinct from the empty-array type sentinel.
  Parameters, returns, arrays, and constant instance/static property defaults
  preserve null tags. Scalar conversions follow PHP's null behavior.
* `is_null`, `is_scalar`, strict comparisons, and `match` distinguish null from
  false, zero, and empty strings. Flow joins and negative scalar guards account
  for the null path instead of incorrectly treating scalar guards as exhaustive.
* `??` and `??=` handle null values, missing keys, and uninitialized mixed
  properties. Receivers and keys evaluate once; fallback expressions are lazy.
* Two CLI tests compare these paths and evaluation order against PHP and reject
  false exhaustiveness — 86 CLI tests total. Example: `examples/nulls.php`.
  Concrete types remain non-nullable; nullable annotations are still planned.

**Delivered (M4.4: containers and dynamic operations).**

* Arrays and objects box at mixed boundaries; concrete arrays can become
  `array<mixed>`. Contextual mixed array literals accept heterogeneous records.
  Nested arrays retain value semantics through assignments, arguments, returns,
  constructors, foreach, and all supported array transforms. Objects share identity.
* `is_array` narrows to `array<mixed>`; `instanceof` narrows to compiled classes
  and interfaces, with nominal facts retained only across valid flow joins.
* Runtime mixed arithmetic, numeric strings, integer-overflow promotion, array
  union, bitwise operators, negation, loose comparison/order, strict array/object
  comparisons, and mixed switch subjects. Native `__toString` dispatch includes
  inherited/overridden methods; invalid signatures fail with `E0275`.

**Delivered (M4.5: unmigrated PHP interop).**

* Leading `// @pxr-dynamic` opts a file into PHP interpretation. Literal
  include/include_once/require/require_once boundaries can execute at top level,
  conditionally, or in functions/methods. Native declaration modules remain AOT.
* A lazily started persistent PHP CLI worker retains include-once state, globals,
  statics, and object identity. Calls into PHP return mixed values; namespace
  lookup, binary data, output order, exceptions, and exit status are covered.
* Value data crosses a binary protocol; PHP objects cross as opaque handles.
  References, resources, native callbacks, and native object transfers fail
  explicitly. Native programs without an executed boundary remain PHP-independent.
  See [`INTEROP.md`](INTEROP.md) for the deployment and language boundary.

**Delivered (M4.6: migration report and acceptance).**

* `pxr migration-report <entry> [--json]` uses the build/check graph and reports
  native/PHP files, line counts, concrete/mixed slots, mixed returns, and dynamic
  call sites. It does not execute PHP. Invalid native code retains diagnostics
  and a failing exit status; JSON identifies schema version 1.
* `examples/migration/main.php` is the acceptance project: compiled typed catalog
  rendering totals a CSV catalog loaded by untyped PHP, alongside globals/statics
  and object operations.
  Tests compare its output with PHP and cover the mixed runtime and boundary
  error paths. There are 99 CLI tests plus 43 unit tests (142 total).

**Acceptance achieved.** All M4 deliverables above run end to end. M4 covers an
explicit migration boundary, not every PHP feature. Compiled union/nullable
annotations, property/element guard caching, and more precise loop analysis
remain outside this milestone. M5 generics and inference are delivered below; async
implementation remains sequenced after M5/M6 in M7.

## ✅ M5 — Generics & inference

**Delivered (M5.1: generic functions).**

* `function identity<T>(T $value): T` with explicit `identity<int>(42)` and
  inferred `identity(42)` calls. Several parameters, nested array patterns,
  concrete/mixed/object arguments, namespace imports, and declaration modules.
* Concrete specialization before IR generation, with one native function per
  function/type tuple. Direct/mutual recursion reuse cached signatures; growing
  polymorphic recursion fails with a bounded specialization diagnostic.
* Per-instantiation body checking, contextual mixed arguments, and existing
  array-copy/object-identity semantics. Unsafe numeric array widening and deep
  element reboxing are rejected. Return annotations remain required.
* Eight CLI tests cover execution, native signatures/reuse, modules, recursion,
  evaluation order, rejected programs, and diagnostics. The full suite has
  107 CLI tests plus 43 unit tests (150 total).

See [`GENERICS.md`](GENERICS.md) and `examples/generics.php`. Generic syntax is a
PXR extension in `.php` files and does not parse in PHP.

**Delivered (M5.2: generic classes and interfaces).**

* `class Box<T>`, `new Box<int>(42)`, and `Box<T>` annotations with concrete
  fields, constructors, methods, and separate static state per instantiation.
* Generic inheritance/interfaces, virtual dispatch, visibility, typed
  `instanceof`, nested types, recursive fields, namespaces, and module aliases.
* Generic functions construct classes from inferred bindings and extract type
  arguments from class/ancestor/interface patterns. Distinct instantiations are
  invariant. Class expansion is cached and bounded, with no PHP dependency.
* Fixed direct typed-object `echo`/`print` to use native magic stringification
  instead of silently emitting nothing.
* Nine additional CLI tests cover execution, inference, inheritance, static
  state, recursive/multiple parameters, mixed/empty arrays, reuse, diagnostics,
  expansion limits, and object printing. The suite now has 116 CLI tests plus
  44 unit tests (160 total), including a parser regression for `instanceof`
  followed by comparison/shift expressions.

**Delivered (M5.3: independent generic methods).**

* Instance/static methods declare their own parameters, with explicit/inferred
  calls, separate class/method arguments, and existing access checks. Generic
  functions and methods share argument inference and concrete type checking.
* Interface/abstract contracts and overrides compare signature shapes up to
  type-parameter renaming. Concrete specializations use existing virtual
  dispatch; parent calls select their implementation directly. Class discovery
  propagates previously requested method specializations to new implementations.
* Lazy body checking, cached specialization, recursion limits, source-context
  diagnostics, receiver/argument order, arrays, and module aliases are covered.
  Class-parameter shadowing, generic constructors/magic methods, and invalid
  contracts fail explicitly. Duplicate/`$this` method parameters are rejected.
* Nine additional CLI tests and one parser regression: 125 CLI tests plus
  45 unit tests (170 total). See `examples/generic_methods.php`.

**Delivered (M5.4: bounds and native traits).**

* `<T: Printable & Named>` bounds on functions, classes, and independent methods,
  checked for explicit and inferred concrete arguments. Nominal class/interface
  ancestry supplies capability contracts; generic/dependent bounds and forward
  parameter references are supported. Overrides preserve bound sets.
* Native PHP-style trait composition for typed properties/methods: nested use,
  namespace/module aliases, private state, consuming-class static storage,
  class/trait/parent precedence, and generic methods with lexical parameter scope.
  Conflicts/cycles and unsupported adaptations fail explicitly.
* Trait bodies use consuming-class checks. Advanced trait adaptations, abstract
  trait requirements, duplicate trait properties, and generic trait declarations
  remain outside this native subset; interfaces supply abstract contracts.

**Delivered (M5.5: whole-function return inference).**

* Omitted function/concrete method annotations infer from all typed return paths,
  across forward calls, direct/mutual recursion, generic function/method
  specializations, class construction, and trait methods. Parameters and
  abstract/interface result contracts retain required annotations.
* Integer/float results widen safely; mixed boundaries, empty arrays, and common
  class ancestors retain their existing representations and value semantics.
  Incompatible paths, missing value returns, ungrounded cycles, and growing
  recursive result types fail with diagnostics. No unresolved type reaches IR.
* Inference and specialization settle together. Provisional specializations are
  discarded when result types change; inheritance contracts are checked against
  final results, including delayed covariant returns. Expansion is bounded.
* Sixteen additional CLI tests and two parser tests cover bounds, traits,
  inference, rejected programs, native signatures, delayed widening, and an
  optimized combined acceptance program running without PHP. The suite has
  **141 CLI tests plus 47 unit tests (188 total)**.

**Acceptance achieved.** Generic functions/types/methods, native specialization,
nominal constraints and trait composition, and whole-function return inference
run end to end. See [`GENERICS.md`](GENERICS.md), [`TRAITS.md`](TRAITS.md), and
[`INFERENCE.md`](INFERENCE.md). Explicit annotations remain necessary where
inference cannot establish one valid result. Default/partial type arguments and
constructor type-argument inference are further extensions, not M5 deliverables.
**M6 is delivered below.** Async remains the next milestone, M7.

## ✅ M6 — Ownership & memory safety

* Owned values and borrows; no leaks, no use‑after‑free, provable at compile
  time; replace the bump arena.
* Affine/linear types, `&`/`&mut`‑style borrowing, lifetimes where needed.

**Delivered (M6.1: runtime allocation lifecycle).**

* Replaced the bump arena with aligned, tracked heap blocks. Strings, arrays,
  objects, mixed boxes, class metadata, PHP handles, and temporary formatting
  buffers share an explicit internal allocation/release path.
* Array/formatting buffer growth releases obsolete buffers immediately; normal
  process return and `exit`-based fatal errors reclaim every remaining block.
  Stable value addresses, array value semantics, object identity, and the LLVM
  value ABI are preserved. Cycles are safe during terminal cleanup.
* Checked allocation arithmetic covers headers, terminators, buffer capacities,
  property slots, repetition, concatenation, replacement, and encoding growth.
* A C runtime harness tests alignment, resizing, copy preservation, cycles,
  overflow rejection, and normal/fatal exit cleanup; it can also run under
  AddressSanitizer and UndefinedBehaviorSanitizer.

**Delivered (M6.2: affine bindings and call-scoped borrows).**

* `owned<T>` local/parameter/result contracts, explicit `move($value)`, inferred
  owned bindings from transfers, optional `drop($value)`, and reinitialization.
  Use after move/drop, implicit sharing, and invalid control-flow availability
  fail with `E0290`, including loop conditions/updates and continue edges.
* `&T` and aggregate `&mut T` parameters with explicit `&$owner` / `&mut $owner`
  arguments. Shared/exclusive loans cover nested argument evaluation, receiver
  calls, reborrowing, mutation, and lifetime escape. Borrows cannot be stored or
  returned as reference-valued handles. Borrowed aggregate bindings cannot be
  rebound; mutation affects their object properties/array elements directly.
* Modules, traits, generics, class ancestors, independent methods, interfaces,
  and overrides preserve ownership modes. Fresh owned classes audit receiver
  escapes from constructors/instance methods. Specialization receives a final
  concrete representation check: borrowing cannot rebox/widen/copy array views.

**Delivered (M6.3: live roots and reclamation during execution).**

* Compiler-inserted stack root frames from backwards CFG liveness after
  optimization. Live slots, SSA pointers, and packed pointer payloads survive
  calls and return transfer. Dead roots disappear across assignment, early
  returns, break/continue, and completion. Explicit drops request collection.
* Allocation-free scalar functions omit root machinery; scalar loops before
  their first possible allocation omit polling. Allocating paths retain precise
  safepoints, with propagation across recursive native calls.
* Precise iterative tracing of arrays, objects, mixed boxes, metadata, and
  statics reclaims unreachable values and cycles. This preserves ordinary PHP
  array-copy/object-identity behavior alongside explicit affine handles.
* Native stringifier re-entry protects C runtime temporaries while collecting
  callback garbage. Native and PHP transfer caches release dead roots while
  preserving live object identity and array lineage. No PHP dependency is added
  to native-only programs.

**Acceptance achieved.** Eight native acceptance tests plus the runtime harness
and focused liveness/representation units verify optimized/unoptimized ownership,
rejected programs, modules/generics/virtual calls, array mutation through borrows,
explicit release before exit, bounded allocation loops/cyclic garbage, deep graph
tracing, callback re-entry, and PHP transfer cleanup. AddressSanitizer and
UndefinedBehaviorSanitizer instrument the runtime and acceptance executables.
The full suite has **149 native CLI tests, one C-runtime harness test, and 52 unit
tests (202 total)**. All examples type-check; optimized ownership/memory examples
run without PHP.

**Scope.** Static owner availability and call-borrow lifetimes are checked before
lowering; shared heap graphs use compiler roots and tracing rather than plain
reference counting. Stored/named-lifetime references, owned fields/nested
qualifiers, scalar variable aliases, native destructors, deterministic resource
cleanup, and thread transfer are further extensions. Shared child object fields
remain shared. These rejected forms are documented in [`OWNERSHIP.md`](OWNERSHIP.md);
M6 does not claim full PHP reference support or `Send`/`Sync` guarantees.
**Next:** M7.1 — async functions and structured tasks.

## 🚧 M7 — Async I/O & concurrency

**Sequencing.** M5 generics and M6 affine ownership/live-root reclamation are
complete. The structured runtime and first native source slice are delivered
below. See [`ASYNC.md`](ASYNC.md) for runnable syntax, current boundaries and
the execution contract.

### 🚧 M7.1 — Async functions & structured tasks

* `async function f(...): T` creates a lazy `Task<T>`; its body returns `T`.
* `await` inside async functions starts or resumes a task and yields its result;
  suspension allows other ready tasks to progress on a single event-loop thread.
* `spawn f(...)` schedules a child task immediately and returns a handle that can
  be awaited. Child tasks cannot silently outlive their parent.
* An `async function main(): void` entry point starts the executor automatically;
  combining it with executable top-level statements is rejected.
* Compiler lowering preserves locals across suspension points. Typed task
  handles, ownership checks, error propagation, and cooperative cancellation
  cover both completion and cleanup paths.
* Begin with nonblocking timers; add readiness-based socket I/O after the task
  lifecycle works. Ordinary blocking calls remain blocking.

**Acceptance.** Native end-to-end tests verify sequential awaits, overlapping
spawned timers, result types, entry-point diagnostics, parent/child lifetime
rules, failure propagation, cancellation, and cleanup. A timer test must prove
that another task progresses while the first is suspended. A socket test must
verify actual I/O readiness without blocking the executor. Async examples are
PXR extensions and are not claimed to run in the PHP interpreter.

**Delivered foundation:** `runtime/pxr_async.c` provides resumable frame callbacks,
a FIFO ready queue, monotonic indexed timers, POSIX descriptor readiness,
structured joins, single-consumer results, error/cancellation propagation, and
cleanup after children. Suspended/lazy frames and completed results contribute
precise roots to the collector. The driver embeds both runtime sources.
Native frame integration tests cover deterministic timer progress, real socket
backpressure/EOF, failure/cancellation cleanup, deep task trees and bounded
completed-task retention at `-O0`/`-O3`, including forced GC and sanitizer builds.
See [`ASYNC_RUNTIME.md`](ASYNC_RUNTIME.md) for the contract and
[`ASYNC_REFERENCES.md`](ASYNC_REFERENCES.md) for cloned primary references.

**Delivered source slice:** free async functions with explicit results,
`await`/`spawn`, `sleepAsync`, and automatic global async main. `Task<T>` is a
concrete internal type; inferred task locals are affine, cannot escape their
creating scope, and cannot be boxed into mixed. Moved owned captures survive
suspension, and call-borrow capture/cross-suspension is rejected. The LLVM backend
lowers optimized async IR to constructors and resumable heap frames with precise
live-root bitmaps, including packed pointer intermediates. Results/locals support
scalars, strings, arrays, shared objects and mixed; native module/name resolution
and ordinary control flow work around awaits. See `examples/async/main.php`.

**Delivered I/O slice:** `tcpExchangeAsync` owns a nonblocking numeric IPv4/IPv6
connection, sends a binary request, half-closes writes, and reads a bounded
response to EOF. Its optional deadline covers the entire started operation.
`cancelTask` consumes a handle and requests structured cancellation. Native source
tests prove progress through protocol gates and cancellation through generated
frames while the peer holds its response open. C tests verify pending read/write/
connect cleanup, descriptor reuse, broken pipes and fake-clock I/O deadlines.
See [`ASYNC_IO.md`](ASYNC_IO.md).

**Remaining:** streaming socket/resource APIs, recoverable error APIs, broader
task/result ownership signatures, generic async functions, async methods and
async return inference. Unsupported source forms
receive diagnostics. The current runtime error envelope does not implement PHP
exceptions or finally/destructors. Threading remains M7.2.

**Validation:** native source tests run at `-O0`/`-O3`, normal/forced GC, and with
AddressSanitizer/UndefinedBehaviorSanitizer. They cover scheduling progress,
ordered awaits, affine handles, joins/cleanup, control-flow suspension, captured
values, module resolution, failure propagation, bounded memory and invalid forms.
All 216 workspace tests, async/heap/ownership sanitizer checks, example checks
and the release build pass.
M7.1 and M7 remain incomplete until the remaining I/O/lifecycle APIs and their
source acceptance checks are delivered.

### ⛔ M7.2 — Threaded parallelism

* OS threads with `Send`/`Sync`‑style guarantees derived from ownership.
* Channels and shared‑nothing tasks; data‑race freedom by construction.
* Explicit cross-thread task transfer rules; async tasks initially stay on their
  event-loop thread. CPU parallelism is separate from async I/O.

## ⛔ M8 — C interoperability

* `extern "C"` declarations, calling C libraries, linking system libraries.
* Exporting a C ABI from PXR; a header generator.

## ⛔ M9 — Tooling & ecosystem

* Package manager, language server, formatter, cross‑compilation, incremental
  builds, `clang` → LLVM‑API backend for in‑process JIT, broader benchmark suite.
