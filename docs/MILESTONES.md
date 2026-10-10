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

## ⛔ M1 — Language breadth I (control flow & data)

* Arrays (indexed and associative) with `[]` literals, indexing, assignment.
* `for`, `foreach`, `do…while`, `break`, `continue`, `switch`, `match`.
* Ternary `? :`, null coalescing `??`, spaceship `<=>`.
* Compound assignment `+= -= *= /= .=` and `++`/`--`.
* Bitwise operators `& | ^ ~ << >>`.

**Acceptance.** A PHP array/quicksort program compiles and matches PHP output.

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
