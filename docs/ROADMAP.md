# Roadmap and design principles

Milestone tracking lives in [`MILESTONES.md`](MILESTONES.md). This document is
the *why*: the principles that constrain every change, and the long‑term shape of
the project.

## The idea

PHP is ubiquitous and approachable, but its runtime is a tree‑walking/Zend
VM interpreter with dynamic typing and reference‑counted values. PXR asks a
different question: *what if the same familiar `.php` source were compiled to
native machine code, with a static type system, memory safety, and zero‑cost
abstractions?*

Two commitments make this practical rather than a rewrite‑everything exercise:

1. **The `.php` extension stays.** Migrating a project does not mean renaming
   files or moving to a new ecosystem. Compilation is a *tool* choice
   (`pxr build main.php`). Files using the shared subset run in either tool;
   PXR extensions such as generic syntax require PXR.
2. **The migration is gradual.** Modern PHP is already mostly typed
   (`function f(int $x): string`). PXR accepts that syntax directly and adds
   strictness; M4 supplies mixed values, guard narrowing, explicit legacy PHP
   boundaries, and a migration report.

## Design principles

* **Make boundaries explicit.** Concrete code receives static diagnostics
  (`E0xxx`); opted-in mixed operations check runtime tags. Marked legacy files
  execute in PHP, never as a silent fallback from a compiler error. Every such decision is listed in
  [`COMPATIBILITY.md`](COMPATIBILITY.md).
* **No feature is claimed until it works and passes tests.** The README and
  milestone docs are written to match the code and the test suite.
* **Measure.** Performance claims come from `benchmarks/run.sh`, and optimizer
  work reports folded/eliminated counts.
* **Small, composable stages.** Each compiler stage is a crate with a narrow,
  testable interface. The IR carries enough type information that later stages
  never re‑derive it.
* **The runtime is ours and small.** Native-only programs run without PHP; opted-in legacy boundaries use a CLI
  worker. The
  runtime is a thin, auditable C library today and grows deliberately
  ([`RUNTIME.md`](RUNTIME.md)).

## Long‑term shape

```
today     typed scalars, arrays, control flow, LLVM backend, tests, benchmarks
  │
  ├─ M1  arrays, loops, switch/match, compound ops        ✅ done
  ├─ M2  owned strings + a real standard library          ✅ done
  ├─ M3  classes, namespaces, compile-time modules        ✅ done
  ├─ M4  mixed values, narrowing, PHP interop, report          ✅ done
  ├─ M5  generics, bounds, traits, return inference        ✅ done
  ├─ M6  affine ownership, borrows, precise reclamation    ✅ done
  ├─ M7  async/await + structured tasks, then threads     (I/O + parallelism)
  ├─ M8  C interoperability (extern "C", FFI)             (systems access)
  └─ M9  package manager, LSP, cross-compile, JIT         (ecosystem)
```

The destination — a compiled, memory‑safe language that speaks PHP — requires
ownership (M6) and generics (M5) as foundations for concurrency (M7) and a
trustworthy standard library (M2). The early milestones exist so the compiler is
*useful* at every step, not only at the end.

## Async sequencing

M7.1 is in progress. Free `async function` declarations with explicit results,
`await`, `spawn`, affine task locals, nonblocking timers and global async main
now compile to resumable native frames. M5 generics and M6 ownership/reclamation
remain foundations; the async compiler extends borrow checks and precise roots
to captures and suspension. [`ASYNC.md`](ASYNC.md) records runnable syntax and
current rejected forms. Public socket operations, source cancellation/error
APIs, async methods/generics and broader task signatures remain pending.
The executor uses one thread; ordinary blocking calls still block it.
Threaded parallelism remains M7.2.

## Non‑goals (for now)

* Re‑implementing the entire PHP standard library or its web SAPI.
* Running unmodified large frameworks (that is what M4's interop and `mixed` are
  for, and even then it is best‑effort).
* Replacing PHP's interpreter for existing deployments — PXR is an additional
  toolchain, not a drop‑in replacement.
