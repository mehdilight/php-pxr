# PXR Architecture

This document explains how `pxr` turns a `.php` file into a native executable,
what each crate is responsible for, and why the LLVM backend is wired the way it
is.

## Pipeline

```
                      .php source (bytes)
                             │
        ┌────────────────────▼────────────────────┐
        │ pxr-lexer   source bytes → tokens        │  diagnostics: E000x/W000x
        └────────────────────┬────────────────────┘
                             │  Vec<Token>, spans (byte offsets)
        ┌────────────────────▼────────────────────┐
        │ pxr-parser  tokens → AST (pxr-ast)       │  diagnostics: E0100
        └────────────────────┬────────────────────┘
                             │  ast::Program (untyped)
        ┌────────────────────▼────────────────────┐
        │ pxr-sema    AST → typed AST              │  diagnostics: E020x/E021x/
        │             + full static type checking  │               E022x/E023x
        └────────────────────┬────────────────────┘
                             │  sema::TProgram (every expr typed)
        ┌────────────────────▼────────────────────┐
        │ pxr-codegen lower: typed AST → PXR IR    │
        │  (coercions, short-circuit, builtins)    │
        └────────────────────┬────────────────────┘
                             │  ir::Module
        ┌────────────────────▼────────────────────┐
        │ pxr-opt     fold / DCE / CFG cleanup     │
        └────────────────────┬────────────────────┘
                             │  ir::Module (optimized)
        ┌────────────────────▼────────────────────┐
        │ pxr-codegen llvm: IR → textual LLVM IR   │
        └────────────────────┬────────────────────┘
                             │  module.ll
        ┌────────────────────▼────────────────────┐
        │ pxr-driver  compile pxr_rt.c (cached)    │
        │             clang module.ll pxr_rt.o     │
        └────────────────────┬────────────────────┘
                             ▼
                      native executable
```

## Crates

| Crate | Responsibility |
| --- | --- |
| `pxr-types` | The shared value type `Ty` (`void/bool/int/float/string/null/named`) and coercion rules. |
| `pxr-diag` | Byte `Span`, `Diagnostic`, `Diagnostics` collection, and the pretty renderer (source line + caret). |
| `pxr-lexer` | Byte‑oriented lexer for the PHP token grammar; emits precise spans and lexical diagnostics. |
| `pxr-ast` | The untyped AST: `Program`, `Stmt`, `Expr`, `FnDecl`, `TypeRef`, operators. |
| `pxr-parser` | Recursive‑descent / precedence‑climbing parser with error recovery at statement boundaries. |
| `pxr-sema` | Scope/variable resolution, type inference, and every static check. Produces the typed AST (`TProgram`). |
| `pxr-ir` | The mid‑level typed three‑address IR and its textual printer. |
| `pxr-opt` | Machine‑independent passes: constant folding, constant‑branch folding, DCE, CFG cleanup. |
| `pxr-codegen` | `lower` (typed AST → IR) and `llvm` (IR → textual LLVM IR). |
| `pxr-driver` | Orchestration, the embedded runtime, toolchain discovery, runtime caching, linking. |
| `pxr` | The CLI. |

The crates form a strict pipeline; each depends only on the stages before it
(`pxr-ir` is self‑contained, `pxr-sema` depends on `pxr-ast`/`pxr-types` only,
and `pxr-codegen` is the only crate that knows about `pxr-sema` *and* `pxr-ir`).

## Why textual LLVM IR (and not `llvm-sys`)?

PXR's backend is LLVM. Rather than linking `llvm-sys`/`inkwell` — which pins the
compiler to an *exact* LLVM development version and requires `llvm-config` — PXR
emits ordinary **LLVM IR assembly** (`.ll`) and hands it to `clang`, which *is* an
LLVM front‑end driver and runs the full LLVM optimizer and machine‑code backend.

Consequences:

* The compiler builds with a plain Rust toolchain; the only external requirement
  is a C compiler on `PATH` (used for the runtime and for linking).
* We get LLVM's `-O` pipeline (including `mem2reg`, `instcombine`, …) for free.
* The generated IR is human‑readable and easy to test (`pxr emit-llvm`).
* The target triple defaults to the host, so cross builds are a matter of passing
  `--target` and a sysroot to `clang` (roadmap).

This is a deliberate milestone‑0 trade‑off. A later milestone can move to a
linked LLVM API for in‑process JIT/`pxr run` without changing the IR contract.

## The IR

The IR is deliberately small and fully typed:

* **Values** (`%vN`) follow SSA rules: every instruction that computes a result
  writes to a fresh value. Function parameters occupy the first `N` value ids.
* **Slots** (`%sN`) are stack allocations used for mutable local variables. The
  entry block holds all `alloca`s, so LLVM's `mem2reg` promotes them to
  registers. This keeps PXR's lowering trivial without requiring φ‑nodes.
* **Blocks** are introduced by `Label(id)`; control flow uses `Jump`/`Branch`.
* **Casts** are explicit (`Inst::Cast`) with a `CastOp` describing the exact
  conversion, so the backend never guesses. String conversions call the runtime;
  numeric conversions map to `sitofp`/`fptosi`/`zext`/`icmp`.

Because the IR carries operand and result types on every instruction, the LLVM
emitter is a near‑mechanical translation and the optimizer needs no type
inference.

### Short‑circuit `&&` / `||`

The IR is φ‑free, so `a && b` is lowered to explicit branches with the result
threaded through a compiler‑private slot:

```
        br i1 %a, label %rhs, label %false
rhs:    %b = <eval b>
        store i1 %b, ptr %tmp
        br label %end
false:  store i1 false, ptr %tmp
        br label %end
end:    %r = load i1, ptr %tmp
```

LLVM's `mem2reg` collapses the slot back into SSA form afterwards.

## The runtime

The runtime (`runtime/pxr_rt.c`) is embedded into the `pxr` binary with
`include_str!`, compiled on demand with the system C compiler, and cached under
`$PXR_CACHE_DIR` (default `~/.cache/pxr`). A program links against that object
plus the generated module, so the resulting executable depends only on the
system C library — **not** on PHP.

See [`RUNTIME.md`](RUNTIME.md) for the ABI.

## Diagnostics

Every span is a byte range. The renderer prints `file: error: [CODE] message`,
the offending source line and a caret underline, plus notes:

```
main.php: error: [E0220] undefined variable `$x`
       |
    1 | echo $x;
       |      ^^
       = note: variables must be assigned before they are used
```

Error codes are stable and documented in [`LANGUAGE.md`](LANGUAGE.md); tests
assert on them.
