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

The same `main.php` can still run under PHP for comparison. Native programs link
only the small PXR runtime; explicitly marked legacy boundaries use a PHP CLI
worker when they execute.

> **Project status: milestones 1–7 complete.**
> M4 adds mixed scalar/null/array/object values, guard narrowing, dynamic
> operators, runtime PHP include/call boundaries, and a migration report.
> `examples/migration/main.php` runs a typed native entry point with untyped PHP.
> M5 adds native specialization for generic functions, classes, interfaces, and
> independent methods; nominal bounds, trait composition, and return inference.
> M6 adds affine `owned<T>` handles, moves, call-scoped borrows, and precise
> reclamation of dead values and cycles. M7 adds an internal executor that
> supports generic async functions/methods, inferred and owned results, affine task handles, await/spawn, nonblocking
> timers, owned TCP exchanges, bounded native TCP servers, consuming cancellation
> and an async main entry. A bounded HTTP adapter adds isolated request globals,
> PHP-style output, header() and http_response_code(), bounded chunked bodies,
> 100-continue and isolated connection/handler failures. `stopServer` adds
> graceful TCP/HTTP shutdown with a deadline for accepted work.
> Owned streaming sockets and TaskResult error recovery complete the I/O layer.
> Explicit OS-thread workers use isolated heaps, ownership-derived transfers and
> bounded duplex channels; see [`docs/PARALLEL.md`](docs/PARALLEL.md) and
> [`docs/ASYNC.md`](docs/ASYNC.md), [`docs/ASYNC_IO.md`](docs/ASYNC_IO.md),
> [`docs/ASYNC_SERVER.md`](docs/ASYNC_SERVER.md), and [`docs/HTTP.md`](docs/HTTP.md).
> See [`docs/OWNERSHIP.md`](docs/OWNERSHIP.md), [`docs/GENERICS.md`](docs/GENERICS.md), [`docs/MILESTONES.md`](docs/MILESTONES.md),
> [`docs/MIXED.md`](docs/MIXED.md), and [`docs/INTEROP.md`](docs/INTEROP.md).

---

## Quick start

Requirements: a Rust toolchain (to build the compiler) and a C compiler on
`PATH` (`clang`; PXR uses it to drive the LLVM backend and link). PHP is **not**
required for native-only programs. Executing a marked legacy PHP boundary
requires a PHP 8+ CLI executable.

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
| `pxr migration-report <file.php> [--json]` | Show native/PHP files, mixed slots, and dynamic call sites |
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
* **Builtins** — `strlen`, `intval`, `floatval`, `strval`, `abs`, `count`,
  string transforms (`strtoupper`, `strtolower`, `trim`/`ltrim`/`rtrim`,
  `substr`, `str_repeat`, `str_replace`, `implode`, `explode`, `str_split`,
  `str_pad`, `substr_count`), predicates & case (`str_contains`,
  `str_starts_with`, `str_ends_with`, `strrev`, `ucfirst`, `lcfirst`, `ucwords`),
  escaping & encoding (`addslashes`, `stripslashes`, `bin2hex`, `base64_encode`,
  `urlencode`, `rawurlencode`, `urldecode`, `rawurldecode`), formatted printing
  (`sprintf`, `printf`), and array helpers (`array_values`, `array_reverse`,
  `array_merge`, `array_slice`, `array_pad`, `array_fill`, `array_key_exists`).
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
* **Objects** — classes, typed properties, methods, constructors, visibility,
  single inheritance, dynamic dispatch, `parent::`, `instanceof`, and abstract
  classes and methods, interface contracts, calls through interface types, and
  static properties/methods via `Class::`, `self::`, and `parent::`.
* **Gradual migration (M4)** — mixed scalars, nulls, arrays and objects;
  scalar/array/instanceof guards, conversions, dynamic arithmetic/comparison,
  value copies, lazy coalescing, and native/PHP `__toString` dispatch. Marked
  legacy PHP files run through persistent include/call interop. The migration
  report distinguishes native and interpreted parts without executing PHP.
* **Modules** — braced and unbraced namespaces, qualified names, class/namespace
  aliases, `use function`, and compile-time `require`/`require_once` dependencies
  with diagnostics pointing to the originating file.
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

Concrete code receives static diagnostics; mixed operations check runtime tags,
and marked legacy files run in PHP. The boundary and differences are documented in
[`docs/COMPATIBILITY.md`](docs/COMPATIBILITY.md). Highlights:

* In strict mode, parameters require annotations; concrete-body returns can be inferred.
  Abstract/interface contracts still require return annotations.
* Using an undefined variable is an **error**, not a warning + `null`.
* Concrete integer overflow wraps; mixed arithmetic promotes to float.
* Nullable and union annotations use checked tagged storage (`?T`, `T|null`, `string|false`).

For native compilation of untyped parameters, bare arrays and changing locals,
use `pxr run --compat main.php`. Explicit types remain checked. See the
[compatibility contract](docs/COMPATIBILITY.md) and
[runnable example](examples/compat-mode/main.php).

## Benchmarks

`benchmarks/run.sh` compares identical programs against the installed PHP CLI,
checking outputs and reporting median wall time after warmup. On the Apple M3
Max with PHP 8.4.25 CLI (JIT disabled), nine runs at PXR `-O3` measured:

| Workload | PXR median | PHP median | PXR relative to PHP |
| --- | ---: | ---: | --- |
| Startup / print | 2.439 ms | 29.073 ms | 11.92× faster |
| Recursive fib(30) | 4.449 ms | 88.389 ms | 19.87× faster |
| Integer loop (5M) | 2.262 ms | 81.774 ms | 36.15× faster |
| Integer-array append + reads (10k) | 2.867 ms | 29.285 ms | 10.21× faster |
| String-key array writes + reads (5k) | 4.223 ms | 29.380 ms | 6.96× faster |
| Read-only array copies (100k × 1k items) | 7.810 ms | 30.433 ms | 3.90× faster |
| Array copy + two overwrites (10k × 1k items) | 16.839 ms | 40.515 ms | 2.41× faster |
| String append (50k) | 3.373 ms | 30.007 ms | 8.90× faster |
| String append with ten snapshots (50k) | 3.135 ms | 29.813 ms | 9.51× faster |
| Objects + methods (2M) | 100.490 ms | 188.720 ms | 1.88× faster |
| Methods on one reused object (2M) | 7.392 ms | 87.851 ms | 11.89× faster |
| Retained objects + reads (100k) | 12.121 ms | 40.530 ms | 3.34× faster |

Timings include startup and exclude compilation. PHP startup on this configuration
is about 29 ms; PXR startup is about 2.4 ms. The latest optimization improved
object creation by 2.9×, repeated method calls by 10.1× and retained objects by
20.0× relative to the previous PXR build. This completes the current optimization
pass; these microbenchmarks are not a general application speedup claim. See the
[object/heap report and raw samples](benchmarks/results/2026-10-16-objects.md),
[string append optimization](benchmarks/results/2026-10-16-string-append.md),
[copy-on-write arrays](benchmarks/results/2026-10-16-array-cow.md),
[earlier array-index optimization](benchmarks/results/2026-10-16-array-index.md),
[original M7 comparison](benchmarks/results/2026-10-16-m7.md), or re-run
`./benchmarks/run.sh` on your machine.

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
keyed `foreach`, `echo` of arrays, nested array assignment, nullable
annotations, overflow promotion for concrete `int` arithmetic, late static binding (`static::`),
advanced trait adaptations and generic trait mixins,
stored/named-lifetime borrows and owned fields, the complex string‑interpolation forms (`"$a[0]"`,
`"{$expr}"`, `"${name}"`, `"$obj->p"`; the simple `"$name"` / `"{$name}"` work),
native exceptions, C interop (`extern`), and broader PHP language/library coverage
in compiled code. Async/worker API boundaries are documented in
[`docs/ASYNC.md`](docs/ASYNC.md) and [`docs/PARALLEL.md`](docs/PARALLEL.md).

## License

MIT. See [`LICENSE`](LICENSE).
