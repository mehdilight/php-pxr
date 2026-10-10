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
   (`pxr build main.php`), so the same file can be run by PHP or compiled by PXR.
2. **The migration is gradual.** Modern PHP is already mostly typed
   (`function f(int $x): string`). PXR accepts that syntax directly and adds
   strictness; untranslatable code gets an explicit `mixed` escape hatch later
   (M4) rather than being forbidden.

## Design principles

* **Fail at compile time, not silently at run time.** If PXR cannot preserve
  PHP's behaviour for a construct, it emits a diagnostic (`E0xxx`) instead of
  compiling something subtly different. Every such decision is listed in
  [`COMPATIBILITY.md`](COMPATIBILITY.md).
* **No feature is claimed until it works and passes tests.** The README and
  milestone docs are written to match the code and the test suite.
* **Measure.** Performance claims come from `benchmarks/run.sh`, and optimizer
  work reports folded/eliminated counts.
* **Small, composable stages.** Each compiler stage is a crate with a narrow,
  testable interface. The IR carries enough type information that later stages
  never re‑derive it.
* **The runtime is ours and small.** Compiled programs run without PHP; the
  runtime is a thin, auditable C library today and grows deliberately
  ([`RUNTIME.md`](RUNTIME.md)).

## Long‑term shape

```
today     typed scalars, arrays, control flow, LLVM backend, tests, benchmarks
  │
  ├─ M1  arrays, loops, switch/match, compound ops        ✅ done
  ├─ M2  owned strings + a real standard library          (usability)
  ├─ M3  classes, namespaces, modules                     (structure)
  ├─ M4  mixed/dynamic + include interop                  (gradual migration)
  ├─ M5  generics + inference                             (zero-cost abstraction)
  ├─ M6  ownership & borrowing, no leaks                  (memory safety)
  ├─ M7  concurrency with data-race freedom               (parallelism)
  ├─ M8  C interoperability (extern "C", FFI)             (systems access)
  └─ M9  package manager, LSP, cross-compile, JIT         (ecosystem)
```

The destination — a compiled, memory‑safe language that speaks PHP — requires
ownership (M6) and generics (M5) as foundations for concurrency (M7) and a
trustworthy standard library (M2). The early milestones exist so the compiler is
*useful* at every step, not only at the end.

## Non‑goals (for now)

* Re‑implementing the entire PHP standard library or its web SAPI.
* Running unmodified large frameworks (that is what M4's interop and `mixed` are
  for, and even then it is best‑effort).
* Replacing PHP's interpreter for existing deployments — PXR is an additional
  toolchain, not a drop‑in replacement.
