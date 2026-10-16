# Async runtime foundation — M7.1 in progress

`runtime/pxr_async.c` implements a single-thread executor tested with native,
hand-written and compiler-generated resumable frames. The driver embeds it
alongside `pxr_rt.c`, so standalone programs need no checkout or external async
library. The first compiler source slice is documented in [ASYNC.md](ASYNC.md).
These internal C interfaces are not a public FFI or a stable generated-code ABI.

## Execution and suspension

Each task has zero-initialized heap frame storage, a resume callback, an optional
synchronous cleanup callback, and an explicit pointer root table. The callback
advances its own program counter and ends every invocation with exactly one
suspension or completion operation. There are no C stack switches or recursive
executor invocations. Only the currently running task can create children.

Tasks start lazy. `pxr_task_start` schedules a task; `pxr_task_await` schedules a
lazy child and suspends its parent until that child completes. Awaiting an
already completed child still yields once. A FIFO queue coalesces wakeups;
`pxr_task_yield` reschedules the current task. No work runs in the task constructor.
The scheduler polls I/O and timers after at most 64 ready callbacks, preventing
a continuously yielding task from starving readiness. A callback that never
returns still blocks the executor. Reentrant and concurrent executors are not
supported.

Timers use monotonic absolute nanosecond deadlines and an indexed minimum heap.
Equal deadlines are ordered by task creation ID. Removing a cancelled timer is
immediate; overflow saturates at the largest deadline, and poll timeouts round
up and clamp to `INT_MAX`. Interrupted waits recompute remaining time. An idle
executor blocks in `poll`, rather than repeatedly polling pending tasks.
Injectable internal clock/wait callbacks allow deterministic timer tests.

`pxr_task_wait_fd` registers one `POLLIN`/`POLLOUT` readiness wait on a nonblocking
POSIX descriptor. It validates descriptor flags and rejects blocking descriptors
and unsupported event masks. Readiness is a hint: the operation must retry
`EAGAIN`/`EWOULDBLOCK`, handle partial transfers, and retry `EINTR`. Hangup/error
readiness wakes the task so its operation can observe EOF or the actual error;
`POLLNVAL` fails the task with `EBADF`. The initial backend rebuilds a `poll`
array from active waits; it does not yet provide an epoll/kqueue backend or
public socket/connect/DNS/TLS APIs. Ordinary file and legacy PHP operations
remain blocking.

## Scope, results and errors

Each task belongs to exactly one parent; the root belongs to its executor.
Handles cannot cross parent scopes. The internal API checks that only a direct
parent awaits a child, and each result has one consumer. A successful result uses
the existing tagged value layout. `pxr_task_take` transfers the result and frees
the child control record. Its caller must publish a returned pointer before any
subsequent collection. Unconsumed completed handles retain their results until
released or until the parent's scope closes.

Normal body completion joins all started children. Lazy children that were never
started are cancelled without executing their body. Dropping a lazy handle has
the same effect; dropping a started handle leaves its work in the parent's scope.
It does not detach the child or suppress its error. A parent cannot complete or
release its frame/resources while children remain active.

The internal result envelope contains `OK`, `CANCELLED`, or `ERROR`, a positive
error code, and the successful tagged result. An operation or child error marks
the parent for failure. The first error **observed by that executor** wins;
errors are not sorted by child creation order, and a later cancellation cannot
replace an error. Remaining children are cancelled and joined. A failed awaited
child fails its parent before the parent body resumes. There is no source-level
catch, task error object, or recovery API yet; this envelope does not introduce
an exception model into PXR. Positive errno values are used by current I/O and
scheduler errors. Non-interrupt poll failures fail the root and drain its tree.
A stalled tree without a runnable task, timer or I/O wait fails with `EDEADLK`.
Allocation failure retains the runtime's abort behavior.

Cancellation is cooperative: the next executor visit ends the task body, removes
its wait, requests cancellation of children, and waits for their cleanup. It
never resumes a cancelled body to run arbitrary user code. Cleanup executes
exactly once after children finish and before frame disposal, including tasks
cancelled before their first resume. Cleanup callbacks may collect, but must not
schedule, suspend, or construct tasks. Generated resource state must distinguish
initialized resources from empty frame fields. Asynchronous cleanup and PHP
`finally`/destructors are not supplied by this internal mechanism.

A descriptor belongs to its operation/resource owner, not to the poll backend.
That owner must keep it open during the wait and close it once in cleanup. The
runtime removes the wait before invoking cleanup, so cancellation cannot leave
a registration referring to a closed or reused descriptor. Generic task cleanup
does not close arbitrary borrowed descriptors.

## Memory and ownership

Executor/control metadata uses explicit native allocation and release. Task
frames use the tracked PXR heap and keep stable addresses. Every registered loop
contributes frame pointers, published pointer slots and tagged result pointers
to the collector, including lazy tasks, suspended tasks, completed results and
frames whose cleanup is running. Raw frames are **not conservatively scanned**;
generated code must update the precise root table before every possible
collection and clear dead entries as values cease to be live.

Cleanup runs while the frame and its roots are still registered. The frame and
root table are then released. Completed records are released on result take,
handle drop, or parent scope exit; there is no ever-growing completed-task log.
Closing the executor releases its root result and internal queue/poll/heap buffers.
The runtime does not support abandoning an active loop; its root must be driven
through completion/cancellation before destruction. Process aborts/signals keep
the existing terminal cleanup limitations.

The source checker rejects call-scoped borrows entering lazy frames, checks
affine task/result consumption and prevents child handles escaping their creating
scope. The first slice uses inferred task locals and rejects generic async
functions, async methods, owned/borrowed async results and async result inference.
OS threads, `Send`/`Sync`, shared tasks and detached tasks remain future work.

## Validation

`cargo test -p pxr --test runtime_async` compiles the real embedded runtime with
native resumable test frames at `-O0` and `-O3`, with normal and forced collection.
`PXR_RUNTIME_SANITIZERS=1` adds AddressSanitizer and UndefinedBehaviorSanitizer.
A per-process alarm bounds scheduler regressions. The harness checks:

* deterministic sequential versus overlapping timers and ordered awaits;
* lazy captured cycles, suspended pointers and completed string results surviving
  collection;
* implicit joins, unused lazy frames, dropped handles and cleanup ordering;
* failure propagation and first-observed error ordering, cancellation before
  first resume and during pending work;
* interrupted/failed event waits and timer overflow/heap removal ordering;
* actual socket-pair readiness, partial writes, backpressure, EOF, and progress
  of a yielding sibling while I/O is pending;
* cancellation of a pending socket wait with exactly-once descriptor cleanup;
* invalid descriptor/event/flag rejection;
* iterative success/error teardown through 2,001 task scopes;
* 2,000 repeated await/take operations with bounded retained frame memory and
  immediate release of completed child control records.

`cargo test -p pxr --test async_source` additionally exercises PHP source through
parsing, typing, ownership, IR optimization, frame lowering and native execution,
including forced GC and optional sanitizers. Public source socket operations and
cancellation/error APIs remain required before M7.1 completes.

## Generated-code bridge

The constructor passes its resume callback to `pxr_async_create`. Frames store
program counter, SSA values and local slots in 64-bit cells. `pxr_async_set`
registers a stored pointer immediately; `pxr_async_publish` atomically replaces
the root table using a compiler liveness bitmap. The backend preserves packed
pointer annotations through frame reloads and stores, so stack safepoints in
mixed/array operations retain those values too. No collection occurs inside the
cell/root helpers. Ordinary generated safepoints still protect the calls.

`pxr_async_await` suspends the current parent; its continuation uses
`pxr_async_take` exactly once. `pxr_async_finish` records a result and begins the
structured join. `pxr_async_spawn` starts a lazy child, while `pxr_async_drop`
releases a handle without detaching started work. `pxr_async_run` drives the CLI
root and drains errors before closing the executor. The compiler currently emits
no source operation for explicitly cancelling an awaited child; the bridge's
successful take assumes the parent was not cancelled or failed, since those
paths do not resume its body. These symbols are internal, not public FFI APIs.
