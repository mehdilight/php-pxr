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
interpolation is rejected at compile time as `E0008`.

## ⛔ M2 — Strings & standard library

* Owned string type (length + data, embedded NUL safe), string interpolation.
* A real standard library (sorted by module) reachable via `use`, compiled from
  PXR sources or intrinsic.
* `sprintf`, `explode`/`implode`, `str_replace`, `substr`, `str_*`, `array_*`.

**Acceptance.** A string‑heavy program (e.g. word count) compiles and matches PHP.

## ⛔ M3 — User types & modules

* `class`, `struct`, methods, visibility, constructors, static members.
* `namespace`/`use`, a compile‑time module/include resolver.
* Interfaces and (single) inheritance with method dispatch (static where
  provable).

**Acceptance.** A small object‑oriented program compiles and runs.

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

## ⛔ M7 — Concurrency

* OS threads with `Send`/`Sync`‑style guarantees derived from ownership.
* Channels and shared‑nothing tasks; data‑race freedom by construction.

## ⛔ M8 — C interoperability

* `extern "C"` declarations, calling C libraries, linking system libraries.
* Exporting a C ABI from PXR; a header generator.

## ⛔ M9 — Tooling & ecosystem

* Package manager, language server, formatter, cross‑compilation, incremental
  builds, `clang` → LLVM‑API backend for in‑process JIT, broader benchmark suite.
