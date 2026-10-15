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

## ⛔ M4 — Gradual migration

* An opt‑in dynamic `mixed` type with runtime tags, and `is_*` narrowing.
* `include`/`require` for unmigrated PHP files (interop layer).
* A migration report: which parts of a project are statically typed vs dynamic.

**Acceptance.** A real (small) PHP project runs compiled, with the untyped parts
using `mixed`.

## ⛔ M5 — Generics & inference

* Generic functions and types (`function max<T>(T $a, T $b): T`).
* Monomorphization in the backend; constraints/traits.
* Whole‑function return‑type inference.

## ⛔ M6 — Ownership & memory safety

* Owned values and borrows; no leaks, no use‑after‑free, provable at compile
  time; replace the bump arena.
* Affine/linear types, `&`/`&mut`‑style borrowing, lifetimes where needed.

## ⛔ M7 — Async I/O & concurrency

**Sequencing.** Implement after typed task support (M5) and ownership/lifetime
rules (M6). Define structured error propagation and cancellation before starting
the runtime implementation. Async is planned, not available in the compiler.
See [`ASYNC.md`](ASYNC.md) for the proposed syntax and execution contract.

### ⛔ M7.1 — Async functions & structured tasks

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
