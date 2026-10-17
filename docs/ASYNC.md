# Async/await and structured tasks

**Implemented:** async functions, methods and by-value closures with explicit or inferred result
types, inferred affine task handles, `await`, `spawn`, nonblocking timers,
owned TCP exchanges/streams, recoverable task results, bounded native TCP/HTTP servers, consuming cancellation and an
async CLI entry point. Programs compile to native resumable frames and run
without PHP. This syntax is a PXR language extension and cannot run in PHP.
Shared-nothing OS-thread workers and bounded channels are described in
[PARALLEL.md](PARALLEL.md). See [MILESTONES.md](MILESTONES.md), the internal
[runtime contract](ASYNC_RUNTIME.md) and [primary references](ASYNC_REFERENCES.md).

Closure and arrow syntax, including bound receivers, is described in
[CALLABLES.md](CALLABLES.md).

## Runnable syntax

```php
<?php
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

Run the checked-in example with `pxr run examples/async/main.php`.

An async declaration's return annotation describes its completed result. Calling
`delayedValue(10)` captures its arguments and creates a lazy `Task<int>`; it does
not execute the body. Argument expressions run when the call is made, in ordinary
call order. Arrays retain PHP value semantics; moved owned arguments transfer
into the task. Await starts a lazy task and produces its completed result.

`spawn` consumes an unstarted task and schedules it, returning a handle whose
awaited result has the same type. Scheduling makes it ready; its body executes
when the executor next runs it. Spawning two tasks before awaiting their handles
permits overlap. Direct consecutive awaits are sequential. Already-started
handles cannot be spawned again. A conditional merge of lazy and started handles
can be awaited, but cannot be spawned because its start state is uncertain.

`sleepAsync(int $milliseconds)` creates a lazy `Task<void>`. Awaiting it suspends
without blocking other ready tasks. Zero milliseconds still yields to the
executor. Negative durations fail when the task starts. `sleepAsync` is reserved
as a PXR intrinsic, including in namespaces; `async`, `await` and `spawn` are
case-insensitive reserved keywords. Await/spawn bind as unary expressions, so
`await value() + 1` adds one to the completed result.

`tcpExchangeAsync` adds a lazy, bounded request/response connection with numeric
IPv4/IPv6 addresses and a whole-operation deadline. `cancelTask($handle)` consumes
a handle and requests cancellation; parent scope exit still joins cleanup. See
[ASYNC_IO.md](ASYNC_IO.md) for exact signatures, protocol, limits and examples.
`tcpServeAsync` adds listening/acceptance and concurrent, bounded async string
handlers using matching EOF framing; see [ASYNC_SERVER.md](ASYNC_SERVER.md) and
`examples/async/server.php`. `httpServeAsync` adds framed HTTP requests, isolated
PHP-style request globals, buffered output and header/status helpers; see
[HTTP.md](HTTP.md) and `examples/async/http.php`.

The initial executor is cooperative and single-threaded. Ordinary blocking
functions and CPU work occupy that thread until they return or await. Use explicit `parallelAsync` for OS-thread CPU work; each worker has its own
executor and heap. Ordinary async handles stay on their creating thread.

## Entry and current source boundaries

The CLI runs global `async function main(): void` automatically. It must have no
parameters. Other declarations and native required modules may accompany it,
but executable top-level statements cannot. Namespace-qualified async functions
are ordinary helpers, not CLI entry points. Synchronous programs retain their
existing top-level entry. Calling an async function, awaiting, spawning or
constructing a timer from synchronous source receives a diagnostic.

Async functions and methods support explicit or inferred result annotations,
including generic functions/methods, inheritance, traits and interface dispatch.
Ordinary synchronous generic functions/methods may also be called
inside an async body. Successful results may be scalar, string, array, shared
native object, mixed, or void. Explicit owned results transfer one owner through
await; borrowed results remain rejected. Inferred returns use the same bounded
fixed-point rules as synchronous functions. Recursive cycles without an anchor
require an annotation. Async main can omit its void annotation but must infer void.

## Generic async functions

```php
async function delayed<T>(T $value): T {
    await sleepAsync(0);
    return $value;
}

async function main(): void {
    $number = spawn delayed(42);
    $word = spawn delayed<string>("hello");
    echo await $number, ":", await $word, "\n";
}
```

Type arguments are inferred from ordinary value arguments or supplied explicitly,
using the existing [generic function rules](GENERICS.md). Arrays, generic classes,
nominal bounds, namespace/function imports and direct/mutual recursion work.
Repeated concrete calls reuse a specialization; each async specialization lowers
to its own lazy constructor and resumable body with precise roots. Unused template
bodies emit no code. Specialization limits and concrete body diagnostics still
apply. See `examples/async/generics.php`.

The written result type describes the completed value, so `: T` creates a lazy
`Task<T>` and `: void` creates `Task<void>`. Generic functions obey the same
borrow, move, cancellation and scope rules as other async functions; they cannot
accept task handles as ordinary generic arguments or return borrowed results. The global async main is never generic. Literal server callbacks still
require a non-generic handler, which can await a generic helper.

## Task handles

Task locals currently use inferred types. Source `Task<T>` annotations, task
parameters/returns, task fields/arrays, shared tasks, escaping handles and detached
tasks are not supported. This keeps every child in its creating async scope.
Task handles cannot be boxed into `mixed`, printed, compared, used as conditions,
or passed to ordinary builtins/generic functions. Rejected forms fail during
checking rather than generating placeholder code.

## Ownership and lifetime

Every task result has one consumer. Await, spawn, move, drop and cancelTask consume
a task variable; later use fails until it is reinitialized. Consume or drop an existing
handle before reinitializing its binding. Moving between locals in
the same scope is supported; ordinary assignment cannot duplicate a handle.
Selecting existing handles in a ternary/match requires explicit moves of the
candidates; both are then treated as potentially consumed after the branch join.

```php
$t = delayedValue(1);
$next = move($t);
$joined = spawn $next;
echo await $joined;
// await $joined; // rejected: result already consumed
```

Instance/static methods use the same lazy-task contract as free functions.
Overrides and interface implementations preserve the async modifier; constructors
and magic methods remain synchronous. An async receiver must be an ordinary
shared object or a moved owner, for example `await move($owner)->read()`.
Call-scoped receiver borrows cannot enter lazy frames. Qualified async calls
cannot capture an owned receiver borrow.

Call-scoped borrowed parameters cannot be captured by async functions, even if
an immediate await would appear safe. Await/spawn/cancelTask/stopServer also cannot cross
an active call borrow during argument evaluation. Owned locals and explicitly moved owned
parameters can survive suspension; synchronous borrow calls are allowed once no
suspension crosses their call lifetime.

A parent joins its started children before completing and releasing its frame.
Unused lazy children are discarded without running. Discarded task-producing
expression statements automatically drop their result; a bare task variable
statement is rejected, so it cannot silently invalidate a live handle.
`drop($lazy)` schedules its
cleanup without executing its body. Dropping a started handle leaves its child
in the parent's scope, where completion and failures are still observed.
Unconsumed successful results are discarded when the parent's scope closes.

## Failure, cancellation and cleanup

An async operation failure propagates to the parent, cancels remaining children,
and waits for their cleanup. A failed awaited child prevents the parent body
from resuming. The first error observed by the executor wins. The CLI reports
`pxr: async task failed: ...` and exits with status 255 after draining the tree.
This currently covers errors reported through the task runtime, such as an
invalid timer duration, TCP transfer failure, size limit or deadline expiration.
Existing synchronous runtime fatal errors retain their process-termination behavior; they are not silently converted into task errors.

The [HTTP server](HTTP.md) installs internal failure boundaries for accepted
connections and handlers. A connection timeout/transport error closes only that
connection; an async handler failure becomes a bounded 500 after descendant
cleanup. Listener and executor failures still propagate. This adds no general
source-level catch syntax and leaves ordinary tasks and raw TCP servers unchanged.

Cancellation is cooperative. The internal runtime removes pending waits and
cleans children before releasing a parent frame. Source cancellation now consumes
handles through `cancelTask`; it does not
synchronously await cleanup or expose a recoverable cancellation result.
`stopServer` consumes a direct TCP/HTTP server handle and requests graceful
shutdown with a bounded period for accepted work before cancellation/join;
see [the server contract](ASYNC_SERVER.md#graceful-shutdown).
Owned streaming sockets and explicit TaskResult recovery are implemented in
[ASYNC_IO.md](ASYNC_IO.md). PHP catch/finally, user-defined native destructors
and arbitrary asynchronous cleanup remain outside this source contract.

## Lowering and validation

After ordinary IR optimization, the backend creates a lazy constructor and a
resume callback for each async function. A program counter selects an await
continuation. The initial implementation stores SSA values and mutable slots
in 64-bit frame cells; it prioritizes correctness over minimal frame size.
Precise liveness bitmaps publish roots for lazy captures, suspended locals and
intermediates. Newly stored pointers are rooted immediately. Packed pointer
payloads keep their pointer markers when reloaded, including array/mixed boxing.
Dead call arguments hand retention to the callee; taking a result releases its
completed child record during the parent's lifetime.

Native CLI acceptance tests exercise actual syntax at `-O0` and `-O3`, normal and
forced GC, plus sanitizer builds. They cover timer progress and ordered awaits,
implicit joins, moved handles, branch joins, control flow around suspension,
owned captures, arrays/objects/mixed/scalar results, module/name resolution,
bounded repeated-result retention, failure cancellation and invalid forms.
Native TCP client/server tests verify concurrent progress, bounded binary transfers,
IPv4/IPv6, handler limits/deadlines, and cancellation through generated parent frames. Deterministic C executor tests
additionally cover readiness/backpressure, descriptor reuse/cleanup, broken pipes
and I/O deadlines. Owned streaming sockets and TaskResult recovery are covered
in [ASYNC_IO.md](ASYNC_IO.md); worker/channel transfers in [PARALLEL.md](PARALLEL.md).
