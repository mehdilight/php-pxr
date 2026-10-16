# M7 implementation references

Read and cloned on 2026-10-16. These are study references, not vendored code or
build dependencies. PXR's executor is an original implementation. No source from
these checkouts has been copied into the runtime.

Local checkouts are outside the project at
`/Users/mehdi/projects/reference-checkouts/php-pxr-async/`. Each was cloned with
`git clone --depth 1`; exact revisions below make the reading reproducible.

| Repository | Revision | License read | Files studied | Application to PXR |
| --- | --- | --- | --- | --- |
| [smol-rs/async-task](https://github.com/smol-rs/async-task) | `f98b30bb047426104c23b81e1c2fa083b177a567` | MIT or Apache-2.0 | `src/state.rs`, `src/raw.rs` | Distinguish queued, running, completed and consumed states; coalesce duplicate wakeups; retain a result until consumption. |
| [libuv/libuv](https://github.com/libuv/libuv) | `d6d3959f0806f5881622c463ce76ae5e1a534b5b` | MIT (`LICENSE`; separate documentation/extra notices also present) | `src/timer.c`, `src/unix/core.c` | Indexed timer removal, absolute deadlines, bounded poll timeouts, readiness instead of blocking operations. |
| [swiftlang/swift-evolution](https://github.com/swiftlang/swift-evolution) | `f006bf3cf2594bdd4a972f9126f93bd1743ed524` | Apache-2.0 | `proposals/0304-structured-concurrency.md` | Child work finishes before the parent scope closes; failure cancels remaining children before parent cleanup. |

Additional primary documentation read:

* [Rust async book: building an executor](https://rust-lang.github.io/async-book/02_execution/04_executor.html):
  tasks become runnable through wakeups; pending work leaves the ready queue.
* [libuv design overview](https://docs.libuv.org/en/v1.x/design.html):
  event-loop phases, timers and readiness-driven network I/O.
* [Swift SE-0304](https://github.com/swiftlang/swift-evolution/blob/f006bf3cf2594bdd4a972f9126f93bd1743ed524/proposals/0304-structured-concurrency.md):
  bounded child lifetime, cancellation and task groups.

PXR deliberately makes its own choices. The first executor is single-threaded,
uses POSIX `poll` without a libuv dependency, has no detached children, and uses
an internal status/error envelope rather than adopting Swift exceptions or Rust
traits. Managed frame roots integrate with PXR's collector. The compiler's
future task types and consumption checker must enforce this runtime contract;
these references do not establish PHP compatibility or source support.

See [ASYNC_RUNTIME.md](ASYNC_RUNTIME.md) for the implemented internal contract and
[ASYNC.md](ASYNC.md) for the pending source language work.
