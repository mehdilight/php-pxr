# PXR — PHP Extended Runtime

**PXR** is a next‑generation *compiled* language built on PHP's syntax and file
extension (`.php`). You keep writing `<?php`, `$variables`, `function`, `echo`
and `while` — but `pxr` compiles the file to a **native machine‑code executable**
through LLVM, with a static type system and compile‑time error detection.

Compilation is selected by the **tool**, not by a new extension:

```sh
pxr build main.php -o main     # -> native executable named `main`
pxr run   main.php             # compile in a temp dir and run
pxr check main.php             # type-check only, no code generation
```

The same `main.php` can still be run by the traditional interpreter for
comparison; PXR programs themselves run **without PHP**, linking only the small
PXR runtime.

> **Project status: milestone 1 complete — language breadth I.** On top of the
> milestone‑0 pipeline (lexer → parser → AST → semantic analysis → type checker
> → IR → optimizer → LLVM backend → linked executable), PXR now has **arrays**
> (ordered maps with `int`/`string` keys), the full C‑style control‑flow set
> (`for`, `foreach`, `do…while`, `switch`, `match`, `break`/`continue` with
> levels), ternary/`??`/`<=>`, compound assignment and bitwise operators. The
> *language surface* is still deliberately small and grows by documented
> milestones. Nothing below is claimed unless it is implemented and covered by
> tests; see [`docs/MILESTONES.md`](docs/MILESTONES.md) and the "Not implemented
> yet" section.

---

## Quick start

Requirements: a Rust toolchain (to build the compiler) and a C compiler on
`PATH` (`clang`; PXR uses it to drive the LLVM backend and link). PHP is **not**
required to run compiled programs.

```sh
cargo build --release            # builds target/release/pxr
export PATH="$PWD/target/release:$PATH"

pxr run examples/hello.php
pxr build examples/fib.php -o fib && ./fib
pxr check examples/types.php
```

Example (`examples/fib.php`):

```php
<?php
function fib(int $n): int {
    if ($n < 2) {
        return $n;
    }
    return fib($n - 1) + fib($n - 2);
}

$i: int = 0;
while ($i <= 15) {
    echo "fib(" . $i . ") = " . fib($i) . "\n";
    $i = $i + 1;
}
```

Note that `function fib(int $n): int` is **valid PHP 7+ syntax** — PXR's typed
subset is a natural extension of modern PHP, which is what makes the gradual
migration path possible.

## CLI

| Command | Description |
| --- | --- |
| `pxr build <file.php> [-o <out>] [--opt 0-3] [--keep-llvm]` | Compile to a native executable |
| `pxr run <file.php> [--args...]` | Compile to a temporary executable and run it |
| `pxr check <file.php>` | Lex, parse, type-check; report errors only |
| `pxr emit-ir <file.php>` | Print the optimized PXR IR |
| `pxr emit-llvm <file.php>` | Print the generated LLVM IR |
| `pxr tokens <file.php>` | Dump the token stream |
| `pxr ast <file.php>` | Dump the parsed AST |
| `pxr version` | Version information |

`-v/--verbose` prints stage timings and optimizer statistics for `build`.

## What works today

* **Front end** — lexer, recursive‑descent parser, AST, precise byte‑span
  diagnostics with source excerpts and carets.
* **Static types** — `int`, `float`, `string`, `bool`, `void`; type inference for
  variables; the single implicit coercion `int` → `float`; `===`/`!==` vs
  `==`/`!=` with distinct rules.
* **Semantic analysis / type checker** — undefined variables and functions,
  arity and argument types, operand types of every operator, return type and
  return‑on‑all‑paths, duplicate functions, required parameter/return types.
* **Builtins** — `strlen`, `intval`, `floatval`, `strval`, `abs`, `count`.
* **IR + optimizer** — typed three‑address IR with an SSA‑style value model;
  constant folding, constant‑branch folding, dead‑code elimination and CFG
  cleanup, with reported statistics.
* **LLVM backend** — emits textual LLVM IR and drives LLVM via `clang` to produce
  a native executable.
* **Standalone runtime** — an embedded C runtime for printing, string
  concatenation, numeric conversion and the array API; compiled output links
  only `libSystem` and needs no PHP interpreter.
* **Control flow** — `if`/`elseif`/`else`, `while`, `do…while`, `for`,
  `foreach`, `switch`, `match`, `break`/`continue` (with levels), blocks,
  `return`, `echo`/`print`, short‑circuit `&&`/`||`.
* **Data** — homogeneous `array<T>` values (PHP ordered‑map semantics) with
  `[...]`/`array(...)` literals, indexing, append, `count`, and `foreach`.
* **Operators** — ternary `? :`, null coalescing `??`, spaceship `<=>`,
  compound assignment (`+=` … `??=`), `++`/`--`, and bitwise `& | ^ ~ << >>`.
  Assignment is statement‑only (no assignment expressions).

## Compatibility with PHP

PXR is *source‑compatible for the shared subset*. `examples/compat.php` is valid
in both PHP 8 and PXR, and the test suite asserts that both produce **byte‑identical
output**:

```
$ php examples/compat.php | diff - <(pxr run examples/compat.php) && echo IDENTICAL
IDENTICAL
```

Where PXR cannot preserve PHP behaviour it **fails at compile time** instead of
silently changing semantics, and the differences are documented in
[`docs/COMPATIBILITY.md`](docs/COMPATIBILITY.md). Highlights:

* Parameters and return types are **required** (PXR is statically typed).
* Using an undefined variable is an **error**, not a warning + `null`.
* Integer overflow **wraps** (two's complement) instead of promoting to `float`.
* `null` is **not implemented yet** and is rejected at compile time.

## Benchmarks

`benchmarks/run.sh` builds each benchmark with `pxr build --opt 3` and compares
against the PHP interpreter (best of N runs). On an Apple‑silicon laptop:

| benchmark | PXR | PHP | speedup |
| --- | ---: | ---: | ---: |
| `fib.php` (naive recursive `fib(30)`) | ~7.5 ms | ~88 ms | ~12× |
| `loop.php` (5M‑iteration integer loop) | ~2.4 ms | ~78 ms | ~33× |

These are illustrative, not a claim about all programs; re‑run them on your own
machine with `./benchmarks/run.sh`.

## Architecture

```
.php source
   │  pxr-lexer        tokens
   │  pxr-parser       AST
   │  pxr-sema         typed AST + all type errors
   │  pxr-codegen      PXR IR (lowering)
   │  pxr-opt          optimized PXR IR
   │  pxr-codegen      textual LLVM IR
   │  clang            object code + link with pxr_rt
   ▼
native executable
```

The workspace is split into focused crates (`crates/pxr-*`); see
[`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md) for the full tour and
[`references/`](references) for the PHP sources used as a behavioural reference.

## Testing

```sh
cargo test          # unit tests for every stage + end-to-end CLI tests
cargo run -p pxr -- run examples/compat.php
./benchmarks/run.sh
```

Integration tests compile and run real native binaries; they skip themselves
automatically when no C compiler is available.

## Not implemented yet

Tracked honestly in [`docs/ROADMAP.md`](docs/ROADMAP.md). The larger items:
keyed `foreach`, `echo` of arrays, nested array assignment, `null`/nullable
types, integer‑overflow → `float` promotion, classes/objects, generics,
ownership & borrowing, string interpolation, exceptions, modules/`include`,
C interop (`extern`), concurrency, and the `mixed`/dynamic escape hatch for
gradual migration.

## License

MIT. See [`LICENSE`](LICENSE).
