# Async/await design — planned for M7.1

**Status: planned; no async syntax or executor is implemented.** This is a PXR
language extension. The examples below are proposed syntax, not runnable PHP or
current PXR programs. Milestone tracking lives in [`MILESTONES.md`](MILESTONES.md).

## Syntax and result types

```php
async function delayedValue(int $value): int {
    await sleepAsync(100);
    return $value;
}

async function main(): void {
    $first = spawn delayedValue(10);
    $second = spawn delayedValue(20);

    $a = await $first;
    $b = await $second;
    echo $a + $b;
}
```

`sleepAsync` is a proposed nonblocking timer API, with delay in milliseconds.
The exact standard-library name is not finalized. It is not an existing builtin.

An async declaration's return annotation describes its completed result. Calling
`delayedValue(10)` creates a lazy `Task<int>` without running its body. Awaiting
that task starts execution and produces an `int`; `spawn` schedules execution
and returns a typed join handle whose awaited result is also an `int`. Scheduling
makes the task ready; execution proceeds when the executor runs it.

`await` is allowed inside async functions. Two consecutive direct awaits are
sequential; spawning two tasks before awaiting their handles permits overlap.
The initial executor is cooperative and runs on one thread. Awaiting a pending
operation suspends that task and lets other ready tasks progress. CPU-heavy work
and ordinary blocking functions occupy the executor thread until they yield or
return. Threaded CPU parallelism is a separate M7.2 feature.

The CLI runs `async function main(): void` using the executor. Declarations can
accompany it, but executable top-level statements cannot. Existing synchronous
programs retain their top-level entry point.

## Task lifetime and failure

The initial design uses structured concurrency: spawned tasks belong to the
parent async invocation. There are no detached tasks in M7.1. A parent waits for
its children before completing normally. On failure or cancellation, it requests
cancellation of remaining children and waits for their cleanup before finishing.

Awaiting transfers a task's result or failure to its caller. An unobserved child
failure must reach the parent rather than disappearing. Task and handle
consumption, result ownership, and the treatment of dropped unawaited tasks must
be specified before implementation. The intended initial model is one consumer
per result; shared tasks are outside the initial scope.

Cancellation is cooperative. Suspension points are candidates for cancellation
checks; tight CPU loops cannot be assumed to stop immediately. The exact cleanup
mechanism and the ordering of simultaneous child failures remain design work.

## Dependencies and implementation order

1. Complete the current class/module milestone (M3) and follow the existing
   roadmap through typed task support (M5) and ownership/lifetime rules (M6).
   General generics and ownership are project sequencing choices; basic async
   execution alone does not inherently require a complete generics system.
2. Specify structured errors and resource cleanup, cancellation propagation,
   task consumption, and parent/child lifetimes. The existing compiler has no
   exception model; async must not silently invent incompatible failure rules.
3. Implement async typing and lowering to resumable frames that preserve locals
   across suspension points, plus the single-thread executor and nonblocking
   timers. Add syntax only as part of a working end-to-end slice.
4. Verify scheduling and lifecycle behavior with deterministic executor tests,
   native CLI tests, and observable timer overlap. Avoid relying solely on tight
   elapsed-time thresholds.
5. Add nonblocking socket readiness and resource cleanup tests. HTTP helpers are
   a later library concern, not an implicit part of the initial syntax feature.
6. Add threaded parallelism under M7.2 with explicit transfer and sharing rules.

## Acceptance checks

* Awaited result types are checked; invalid operands and await outside async
  contexts receive compile-time diagnostics.
* Sequential awaits preserve order; spawned pending timers allow other tasks to
  progress, including when handles are awaited in creation order.
* Locals and owned values survive suspension and are released on completion,
  cancellation, and failure.
* Parents cannot finish while child tasks are still running; child failures are
  observable and cancellation cannot silently leak children or I/O resources.
* Async entry points run without PHP; ambiguous entry points are rejected.
* Socket operations wait for readiness without blocking unrelated tasks.
* Existing synchronous programs and their tests continue to pass.
