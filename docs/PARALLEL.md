# Shared-nothing parallel workers and channels

M7.2 adds native POSIX threads without sharing the managed heap. Async tasks
remain on their creating event loop. Explicit `parallelAsync` starts a separate
worker when its lazy task runs; `spawn parallelAsync(...)` starts it concurrently.

```php
async function square(int $n): int { return $n * $n; }
async function main(): void {
    $a = spawn parallelAsync("square", 6);
    $b = spawn parallelAsync("square", 7);
    echo await $a, ":", await $b; // 36:49
}
```

The callback is a literal fully qualified native **non-generic async free function
with one parameter**. Its ordinary async helpers, methods and generic calls run
inside the worker. The completed callback result is the result of `parallelAsync`.
Arguments are evaluated and copied/transferred when constructing the lazy task.
A callback name in a namespace must include that namespace; it is not dynamically
resolved or called through PHP. See `examples/async/parallel.php` and
`examples/async/parallel_values.php`.

## Transfer contract

The ownership checker derives the boundary's Send permission:

* Scalars, null, strings and value arrays cross by copy. Mixed and arrays of mixed
  undergo recursive runtime validation before publication. Object payloads hidden
  in mixed receive an async EINVAL error; no heap pointer is sent to another thread.
* A moved `owned<X>` native object crosses by deep copy and ownership transfer when
  its instance fields contain only scalar/string/array/mixed values. Inherited and
  generic instance fields are checked too. The callback must declare an owned
  object result to transfer an object back. Nested objects, shared object roots,
  borrowed parameters/results, sockets, Task and TaskResult boundaries are rejected.
* A moved `owned<Channel>` crosses as a retained native queue endpoint token. The
  original heap proxy closes at construction; the destination creates its own proxy.
  It can be returned as an owned result. Channel endpoints cannot be embedded in
  messages, arrays or object fields. This prevents reference cycles between queues.
* Ordinary async task handles never cross threads. Workers create their own task
  trees and must join them. There is no implicit migration, detached task, arbitrary
  shared object or user-defined unsafe Send/Sync override.

Each worker has its own heap, precise GC roots, static property storage, class
registrations, HTTP/CLI globals and executor. Static defaults are initialized in
that worker; mutations are not visible to the parent or other workers. HTTP
request globals do not transfer automatically: send the needed values explicitly.
Immutable compiled literals/code are shared. The channel's internal C queues and
reference counts are protected by a mutex; completion/cancellation flags and
worker accounting use C11 atomics. This is the runtime's Sync boundary.

One transfer has a 1 MiB allocation budget, including message metadata, and a
64-level recursion limit. Oversized/deep values fail with EMSGSIZE; values are
never truncated. At most 64 worker tasks can hold OS-thread slots process-wide;
excess starts fail with EAGAIN. Worker failures propagate like ordinary task
failures, or can be handled with `taskResultAsync(parallelAsync(...))`.

Programs using parallel workers cannot also use dynamic PHP module interop.
That combination receives a compiler diagnostic because starting a PHP subprocess
would use process-wide fork/signal machinery while other threads may be running.
Native PHP-style builtins, classes and HTTP helpers remain usable.

## Bounded duplex channels

```php
async function worker(owned<Channel> $peer): void {
    $n = intval(await channelReceiveAsync($peer));
    await channelSendAsync($peer, $n * 2);
    closeChannel($peer);
}
async function main(): void {
    $channel = await channelAsync(1);
    $peer = await channelPeerAsync($channel);
    $task = spawn parallelAsync("worker", move($peer));
    await channelSendAsync($channel, 21);
    echo await channelReceiveAsync($channel); // 42
    await $task;
    closeChannel($channel);
}
```

| Operation | Completed result / behavior |
| --- | --- |
| `channelAsync(int $capacity = 64)` | `owned<Channel>`, capacity 1–1024 per direction |
| `channelPeerAsync($owner)` | Owned endpoint on the opposite side |
| `channelCloneAsync($owner)` | Owned endpoint on the same side |
| `channelSendAsync($owner, $value, int $timeoutMs = 30000)` | `void`; waits for space, enqueues a copied value |
| `channelReceiveAsync($owner, int $timeoutMs = 30000)` | `mixed`; waits for and removes one value |
| `closeChannel($owner)` | Consumes the endpoint, closes its reference |
| `drop($owner)` | Consumes and closes the endpoint |

Async operations require an owned local endpoint and do not consume it. Messages
are scalar/string/array data; statically known objects receive diagnostics,
dynamic object payloads fail with EINVAL. Use `is_array`/other existing guards to
narrow received mixed values. Channels deliberately have no generic type argument.
Send and receive deadlines start at first execution and are absolute across
wakeups. Zero timeout tries the queue immediately; a negative timeout fails.
One send and one receive may be pending on an endpoint; a second operation in the
same direction fails with EBUSY. Clone endpoints for multiple producers/consumers.
The two directions are independent FIFO queues. A pending send contributes one
bounded private message in addition to enqueued messages.

Closing the last reference on a side wakes the other side. Buffered messages can
still be received, then receives fail with EPIPE. Sending to a closed side fails
with EPIPE. Creating a peer after that side has closed also fails; closure is
permanent. The initially uncreated peer is not considered closed. Closing one clone
leaves its siblings usable. Explicitly closing an endpoint with pending operations
fails those operations with ECANCELED. `TaskResult` can recover these errors.
Scope exit closes endpoints after joining children; queues/pipes are freed after
the final reference, including worker transfer tokens and unclaimed results.

## Cancellation and verification

Cancellation signals a worker's event loop through a nonblocking pipe. The
worker cancels and joins its task tree and releases its heap/resources before
publishing completion. The parent polls completion without blocking its executor;
`pthread_join` runs only after completion is published. A dropped lazy worker task
never creates a thread. A dropped started handle stays structured under its parent.
CPU-only code must return or await before its worker can observe cancellation;
this is cooperative cancellation, without unsafe pthread termination.

Source acceptance runs at O0/O3 with normal/forced GC and ASan/UBSan. Native tests
assert actual distinct pthreads, independent class/static/heap state, cancellation
join order, queue/message limits, resource counts and descriptor closure. The same
native tests can run under ThreadSanitizer with
`PXR_RUNTIME_THREAD_SANITIZER=1 cargo test -p pxr --test runtime_parallel`.
Primary reference provenance is in [ASYNC_REFERENCES.md](ASYNC_REFERENCES.md).
