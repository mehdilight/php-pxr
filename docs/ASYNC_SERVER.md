# Native async TCP server

PXR can listen on a numeric IPv4/IPv6 address, accept connections and run native
async handlers concurrently. This first server API implements bounded,
EOF-delimited TCP requests. HTTP parsing, routing, request/response objects,
streaming handles and PHP request superglobals are not implemented.

```php
<?php
async function reply(string $request): string {
    await sleepAsync(0);
    return "reply: " . $request;
}

async function main(): void {
    // Zero means keep accepting until this server task is cancelled.
    await tcpServeAsync("127.0.0.1", 8766, "reply", 0, 4096);
}
```

`pxr run examples/async/server.php` runs a finite, self-contained demonstration:
a native listener and two PXR clients share the executor, exchange replies, and
exit after both connections finish. It needs no PHP worker or Python fixture.
The example uses loopback port 8766, which must be available.

## Source contract

`tcpServeAsync(string address, int port, literal string handler, int maxRequests,
int maxBytes, int timeoutMs = 30000, int maxConcurrent = 64)` creates a lazy
`Task<void>`. The last two arguments are optional; this does not introduce
user-defined default parameters. Like other async intrinsics it requires an
async function context, supports await/spawn/drop/cancelTask, and is reserved
case-insensitively, including in namespaces and function imports.

The handler must be a literal name of a native, non-generic free async function
with one string parameter and a string result. An `owned<string>` parameter is
also safe: each invocation receives a fresh request string. Async borrowed
parameters and owned results remain rejected by the existing source contract.
No dynamic function lookup, PHP worker dispatch, callable arrays, closures or
method callbacks are supplied. Parentheses around a literal are allowed.
Function names are case-insensitive; namespaced handlers use their full name,
such as `"App\\reply"`, optionally with a leading backslash. Strings do not
resolve namespace-relative names or `use function` aliases. The intrinsic itself
can be imported with `use function tcpServeAsync as serve`.

The listening address must be a numeric IPv4/IPv6 literal and the port must be
1–65535. IPv6 listeners explicitly accept IPv6 only; use separate tasks to serve
both address families. DNS, scoped IPv6 literals, URLs and TLS are unsupported.
Listener and accepted sockets are nonblocking and close-on-exec. Accepted sockets
receive their flags explicitly; they do not rely on listener inheritance.

`maxRequests` counts accepted connections, rather than successful responses.
A positive value stops accepting after that many connections, closes the
listener, then joins the accepted connections and their handlers. Zero keeps
accepting until cancellation or failure. Negative values fail with `EINVAL`.
An idle continuous server intentionally waits indefinitely for new connections.

`maxConcurrent` must be 1–1024. It bounds active accepted connection tasks,
including request reads, handler execution, handler descendants and response
writes. At capacity the server suspends until a child finishes; it does not
spin or continue accepting into an unbounded user-space queue. Pending kernel
connections are subject to the listen backlog, requested at the concurrency
limit and capped by `SOMAXCONN`; the operating system may further limit it.
Completion releases each internal connection handle during the server lifetime.

## Framing, limits and deadlines

Each accepted connection carries one binary-safe request. The peer sends its
request, closes its write side, and then reads the response until the server
closes the connection. PXR's `tcpExchangeAsync` is a matching client. The server
invokes the handler only after request EOF, including for an empty request.
It writes the complete returned string and closes the connection. Partial reads,
partial writes, interrupted transfers and readiness races are handled. Broken
pipes use the existing per-socket/send SIGPIPE protection.

This framing is not HTTP: browsers typically wait for a response without first
half-closing their connection. Sending an HTTP-looking string does not change
the protocol. Persistent connections and arbitrary full-duplex streams require
later resource APIs.

`maxBytes` is a nonnegative, independent content limit on both the request and
the handler's response. Exceeding either fails with `EMSGSIZE`, without silently
truncating data. Reaching the request limit still requires EOF. Zero permits only
an empty request and empty response. Private buffer capacity, captured arguments,
string copies and handler allocations also consume memory; this is not a bound
on all memory the handler may allocate. An oversized handler response is rejected
before sending it, but its allocation has already occurred.

`timeoutMs` must be positive. Each connection has one absolute monotonic deadline
starting when it is accepted, covering request reads, handler execution and its
structured children, and response writes. Readiness, partial progress and handler
suspension never reset it. A timer remains armed while awaiting the handler;
expiration cancels and joins that subtree before socket cleanup. No timer limits
how long the listener waits for a new connection. The executor is cooperative,
so blocking calls and uninterrupted CPU work can delay enforcement.

## Failure and cancellation

The task tree is `caller -> server -> connection -> handler -> handler children`.
Descriptors stay inside runtime-owned tasks. Normal finite completion joins all
accepted work. Cancelling the server stops its body and cancels/joins connection
and handler descendants before releasing its resources. A cancelled lazy server
never opens a listener. Cleanup closes initialized sockets exactly once and frees
private buffers, including on transfer errors, invalid configuration, bind/listen
failure, timeout and cancellation. It never retries close on a reused descriptor.
Captured strings and completed handler results remain precise GC roots.

The first observed task error fails the server, cancels remaining work, and
propagates to its caller. A malformed configuration reports `EINVAL`; timeout
reports `ETIMEDOUT`; other socket errors keep their errno. The CLI drains the
tree, prints `pxr: async task failed: ...`, and exits 255. This first slice has
no per-connection error recovery. Recoverable errors and graceful shutdown with
a drain deadline are still future work; cancellation stops active handlers.
Cancellation cannot retract bytes already transmitted or undo handler effects.

## Next server layers

Streaming resource ownership and recoverable errors remain M7 work. The HTTP
layer will need request framing and limits, request/response types, routing, and
request-local query/form/cookie/upload/header access. PHP compatibility
superglobals such as `$_GET`, `$_POST`, `$_COOKIE`, `$_FILES`, `$_SERVER`,
`$_REQUEST` and `$_SESSION` are not populated by this TCP API. Any later
compatibility layer must isolate concurrent requests rather than share mutable
process globals. Neither sessions nor an HTTP API is claimed by this slice.

## Validation

Native source tests run at `-O0`/`-O3`, with normal and forced GC and optional
ASan/UBSan. A protocol gate holds one handler until another response has been
awaited, proving concurrent progress without a short elapsed-time assertion.
Tests cover concurrency one, binary 1 MiB requests/responses, IPv4/IPv6,
empty EOF, byte limits, handler and bind errors, invalid configuration/signatures,
namespace/import resolution, suspended-handler cancellation, repeated listener
cancellation/rebinding, and finite joins.

C executor tests cancel partial reads, writes under backpressure, suspended
handler trees and idle/lazy listeners. They assert exactly-once cleanup and
closed descriptors before executor destruction, test descriptor reuse under
collection, and use a fake clock to expire an awaiting connection's deadline.
Primary reading is recorded in [ASYNC_REFERENCES.md](ASYNC_REFERENCES.md).
