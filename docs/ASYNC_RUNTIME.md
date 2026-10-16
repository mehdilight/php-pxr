# Structured async runtime

`runtime/pxr_async.c` implements a single-thread executor tested with native,
hand-written and compiler-generated resumable frames. The driver embeds it
alongside `pxr_rt.c`, `pxr_http.c`, `pxr_stream.c` and `pxr_parallel.c`, so standalone programs need no checkout or external async
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
returns still blocks the executor. Reentrant executors and concurrent use of one executor are rejected. Explicit
workers run independent thread-local executors/heaps.

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
array from active waits; it does not yet provide an epoll/kqueue backend, DNS
or TLS. Owned streaming socket handles use the same readiness backend. The first source operation,
`tcpExchangeAsync`, owns one connection and is documented in
[ASYNC_IO.md](ASYNC_IO.md). Ordinary file and legacy PHP operations remain blocking.

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
error code, and the successful tagged result. An ordinary operation or child error marks
the parent for failure. The first error **observed by that executor** wins;
errors are not sorted by child creation order, and a later cancellation cannot
replace an error. Remaining children are cancelled and joined. A failed awaited
child fails its parent before the parent body resumes. Source `taskResultAsync` installs an isolated child boundary before execution
and returns an owned TaskResult after cleanup. `resultValueAsync` consumes it
and either transfers its payload or propagates its error; see [ASYNC_IO.md](ASYNC_IO.md).
This does not introduce a PHP exception model. Positive errno values are used by current I/O and
scheduler errors. Non-interrupt poll failures fail the root and drain its tree.
A stalled tree without a runnable task, timer or I/O wait fails with `EDEADLK`.
Allocation failure retains the runtime's abort behavior.

Runtime-owned scopes can set `isolate_failure` before starting a task. Its failure
still cancels/joins its descendants and runs cleanup exactly once, but completion
does not mark its parent failed. Awaiters receive the original error envelope;
dropped handles remain scoped/joined and capacity waiters still wake. The flag
does not propagate to descendants and does not suppress parent cancellation or
executor failures. It is internal; source recovery is the explicit taskResultAsync wrapper.
HTTP sets it on accepted connections and their handlers: connection errors close
only that socket, and an awaited handler error can become a bounded 500 after join.

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
scope. Free async generic functions specialize to concrete functions before the
existing resumable lowering, preserving async identity in signatures and wrapping
call results as tasks. They use the same precise live-root analysis and task ABI;
no separate generic runtime or boxed dispatch is introduced. The source uses
inferred task locals, async methods (including generic/virtual methods), inferred
results and explicit owned results. Owned result qualifiers are preserved by the
source ownership pass and await transfers the result exactly once. Borrowed
results and borrowed async parameters/receivers remain rejected.
Explicit OS-thread workers use derived Send checks and synchronized native
channels; see [PARALLEL.md](PARALLEL.md). Shared/detached task handles remain rejected.

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
including forced GC and optional sanitizers. Source TCP clients/servers and
consuming cancellation, owned streaming resources, TaskResult recovery and
isolated workers/channels now use this runtime. The native parallel suite checks
thread identities, TLS heaps/statics, cancellation joins and resource lifetimes.

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
root and drains errors before closing the executor. `pxr_async_cancel` consumes an unclaimed lazy/spawned handle and requests
cancellation. The source checker disallows later awaits of that handle; the
bridge's successful take assumes the parent was not cancelled or failed, since
those paths do not resume its body. These symbols are internal, not public FFI APIs.

The TCP operation roots its address, request and private response buffer and owns
its descriptor until cleanup. Pending I/O can also have an indexed deadline
timer. Waking for readiness, deadline, error or cancellation removes that timer;
the next I/O wait rearms the same absolute deadline. Cleanup runs after the wait
is removed, closes once, and clears/releases the private buffer. No descriptors
cross the generated value ABI.


The bounded server (`pxr_async_tcp_serve`) receives a statically checked native
async constructor address, not a dynamic callable. Each accepted connection is
a started, internally released child; its string handler is an awaited child.
A server at its concurrency limit enters `PXR_TASK_CHILDREN`, a wait for any
direct child completion. Completion frees a slot and wakes the server once,
without retaining completed connection handles or polling a full listener.

`pxr_async_stop_server` consumes a direct server handle and wakes it into a
draining phase. That phase closes the listener before waiting for children and
registers one absolute timer in `PXR_TASK_CHILDREN`. Child completion removes the
wait/timer and the next visit rearms the same deadline if necessary. Expiry
requests server cancellation, retaining the ordinary join-before-cleanup rules.
A finite server already in normal joining can re-enter the draining phase while
its frame is still alive; existing errors/cancellation are never cleared. Invalid
arguments queue an error child rather than interrupting a running generated
callback midway through an ordinary intrinsic call.

A connection's absolute deadline can also be registered while awaiting its
handler in `PXR_TASK_CHILD`. Handler completion, cancellation and timer expiry
remove that registration through ordinary wakeup coalescing. If the timer wins,
the connection fails and drains the handler subtree before closing its socket;
it does not attempt to take an unfinished handler result. The request copy is
explicitly rooted while its generated constructor can collect. The lazy handler
then retains its parameter, and a consumed handler response is rooted until the
last partial write completes. [ASYNC_SERVER.md](ASYNC_SERVER.md) describes source
configuration, finite joins, fail-fast errors and protocol limitations.


`pxr_http.c` adapts those same server/connection tasks to bounded HTTP framing.
A zero-argument native async handler inherits a fresh `pxr_http_context` through
`pxr_task_new`; synchronous helpers access the currently running task's context.
All request arrays, header arrays, raw body and output buffer are explicit roots,
including while descendants join or cancel. The context is raw managed storage,
so those fields are traced deliberately, rather than scanned conservatively.
A separate rooted fallback supplies initially empty CLI request arrays and is
never used to initialize an accepted HTTP request. Response serialization waits
for handler descendants; `echo`/`print`/`printf` can therefore stay buffered and
headers mutable until completion. HTTP protocol errors become local responses;
handler failures become bounded 500 responses after descendant cleanup, while
connection transport/deadline failures close only their own connection. Listener
and executor failures still propagate. Ordinary tasks and raw TCP servers retain
their fail-fast policy. After
sending, HTTP tasks shut down writes and use a bounded drain phase. An expired
drain completes successfully, while an expired application/read/write wait still
fails with ETIMEDOUT. [HTTP.md](HTTP.md) specifies all source/protocol boundaries.

## Resource and worker ownership

Each task roots an explicit native-resource list. Native objects are heap-local
proxies for owned C control records. A generated async constructor adopts resource
parameters, and successful task results promote a resource to the parent before
joining cleanup. TaskResult payload traversal preserves this behavior through
recovery wrappers. Explicit value drop, unclaimed result destruction and failed
completion discard resource results. Pending read/write leases keep closed
control records alive until their operations have removed waits and cleaned up.

Workers serialize data into bounded malloc-owned trees without managed pointers.
Their completion state is published atomically after loop/resource/heap cleanup;
the parent joins only after this publication. Deferred cancellation prevents a
parallel task from completing while its OS thread still owns data. Cancellation
pipes participate in worker polling, including nested parallel joins. Channels
hold serialized values under a mutex and nonblocking pipe readiness hints; there
is no managed shared heap. [PARALLEL.md](PARALLEL.md) records the public contract.
