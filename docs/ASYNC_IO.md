# Async TCP connections, streams and cancellation

PXR provides a first source-level TCP operation with an owned connection:

```php
async function main(): void {
    $reply = await tcpExchangeAsync("127.0.0.1", 8765, "hello", 4096, 5000);
    echo $reply, "\n";
}
```

The intrinsic accepts `(string address, int port, string request, int maxBytes,
int timeoutMs = 30000)` and creates a lazy `Task<string>`. The fifth argument is
optional. This is its API contract; task type annotations and user-declared
default parameters are not introduced by this intrinsic.

Run a local two-request example in separate terminals:

```sh
python3 examples/async/tcp_server.py
pxr run examples/async/tcp.php
```

Both files use loopback port 8765. The fixture exits after serving two requests.
No PHP CLI, DNS service or external network endpoint is needed.

## Protocol and bounds

The operation opens a nonblocking TCP connection, writes the entire request,
shuts down its write side, and reads a response until peer EOF. Its result is a
binary-safe PXR string, including embedded NUL bytes. The peer protocol must
accept request EOF and then finish its response by closing its send side. This
is a request-then-response operation. Use the separate stream API below for persistent or full-duplex protocols.

Addresses must be numeric IPv4 or IPv6 literals; pass `"::1"` without URL brackets
for IPv6 loopback. Hostnames, scoped IPv6 addresses, URLs, TLS and DNS remain
outside these APIs. Accepted handles, persistent connections and streaming
read/write use the separate resource API below. Native listening/acceptance with
EOF-framed handlers is also available through
[`tcpServeAsync`](ASYNC_SERVER.md), with the same EOF-delimited protocol. Invalid addresses and embedded NULs fail with
`EINVAL` when the task starts. Ports must be 1–65535.

`maxBytes` is a nonnegative response-content limit. Exceeding it fails the whole
task with `EMSGSIZE`; it never returns a silently truncated response. Reaching
the limit still requires EOF to succeed. Zero accepts only an empty response.
Private buffer capacity grows as needed; transient memory also includes that
capacity, captured request bytes and the completed result copy. The limit is not
a promise that total allocation equals response length. Allocation failure keeps
the runtime's abort behavior.

`timeoutMs` bounds the whole operation from its first executor poll, including
connect, writes and response reads. Waiting in a lazy task does not consume that
time. Negative durations fail with `EINVAL`; zero expires before opening a
socket. Expiration reports `ETIMEDOUT`. Readiness and an absolute monotonic timer
share the same task; whichever wakes it removes the other registration. A new
wait uses the same deadline, so partial transfers do not restart the timeout.
The cooperative executor checks the deadline when the task resumes; a blocking
ordinary call elsewhere can delay that check.

Connect completion checks `SO_ERROR`, rather than assuming writability means
success. Partial writes, short reads, `EAGAIN`/`EWOULDBLOCK`, and interrupted
transfers are handled. Callback work is bounded so a continuously ready peer
cannot starve other tasks. Broken pipes report task errors without changing the
process-wide SIGPIPE disposition. Socket creation configures nonblocking and
close-on-exec flags before connecting. All connection state remains inside the
operation; no descriptor is exposed through PXR values.

## Cancellation and resources

```php
async function main(): void {
    $request = spawn tcpExchangeAsync("127.0.0.1", 8765, "hello", 4096);
    await sleepAsync(10);
    cancelTask($request);
}
```

`cancelTask` is a reserved async-context intrinsic returning void. It consumes a
lazy or spawned task handle and requests cooperative cancellation. The handle
cannot subsequently be awaited, cancelled, copied or otherwise reused. A fresh
task expression can also be cancelled directly. Cancelling a completed successful
handle discards its result. Cancellation cannot erase a task error already
recorded by the executor.

The request itself does not suspend or synchronously wait for cleanup. The
parent still joins the cancelled task and its descendants before its scope
finishes. Cleanup removes pending waits, closes an initialized socket exactly
once, and releases private response buffers, including on timeout, transfer
error, cancellation and normal EOF. A cancelled lazy task never opens a socket.
An awaited successful result is delivered only after the operation closes its
connection. Captured strings remain rooted across all waits and cleanup.
Cancellation and deadlines cannot retract bytes already sent or undo peer actions.

Cancellation consumes the handle, so this API does not permit awaiting a
cancelled handle or catching a cancellation result. Recoverable error objects,
catch/finally, async cleanup and public socket ownership types remain future
work. Other task errors still propagate, drain the structured tree, and cause
the CLI to report `pxr: async task failed: ...` with exit status 255. Existing
synchronous fatal errors retain their process-termination behavior.

For TCP/HTTP listener tasks, [stopServer](ASYNC_SERVER.md#graceful-shutdown)
adds graceful shutdown with a deadline before cancelling unfinished accepted work.

`sleepAsync`, `tcpExchangeAsync`, `tcpServeAsync`, `cancelTask` and `stopServer` are case-insensitive reserved
intrinsics. Their lookup works in native namespaces and through function imports.
Calls with incorrect arity/types, calls from synchronous source, repeated handle
use, and borrowed values escaping into an operation fail during compilation.

## Validation

The native source suite tests overlapping exchanges with a protocol gate: the
server withholds one response until another response was awaited and a third
connection was opened. This proves progress without a tight elapsed-time test.
It also covers binary requests/responses, a 1 MiB request, exact size limits,
IPv4/IPv6, empty EOF, over-limit failure, invalid addresses/ports/durations,
consumed-handle diagnostics, and cancellation through a generated parent frame
while the peer keeps its response stream open.

C executor tests force actual write backpressure with small socket buffers,
cancel partially buffered reads and connects, check exactly-once cleanup and
safe descriptor reuse, and verify broken-pipe errors with default SIGPIPE.
A fake-clock test proves deadline expiration during an I/O wait without sleeping.
The suites run at `-O0`/`-O3`, with normal/forced GC and optional AddressSanitizer
and UndefinedBehaviorSanitizer. See [ASYNC_REFERENCES.md](ASYNC_REFERENCES.md) for
primary implementation references and [ASYNC_RUNTIME.md](ASYNC_RUNTIME.md) for
the internal task contract.

## Owned listeners and streams

`TcpListener` and `TcpStream` are abstract native resource types. User code cannot
construct or extend them, box their owners into mixed, borrow them across awaits,
or transfer them to an OS-thread worker. Native factories return affine owners:

| Operation | Completed result |
| --- | --- |
| `tcpListenAsync(string $address, int $port, int $backlog = 64)` | `owned<TcpListener>` |
| `tcpAcceptAsync($listener, int $timeoutMs = 30000)` | `owned<TcpStream>` |
| `tcpConnectAsync(string $address, int $port, int $timeoutMs = 30000)` | `owned<TcpStream>` |
| `tcpReadAsync($stream, int $maxBytes, int $timeoutMs = 30000)` | `string` |
| `tcpWriteAsync($stream, string $bytes, int $timeoutMs = 30000)` | `int`, bytes written |
| `tcpShutdownWrite($stream)` | Synchronous half-close after pending writes finish |
| `closeSocket($owner)` | Consumes and closes the listener/stream |
| `drop($owner)` | Consumes and closes the resource |

Factories and I/O require an async context. Half-close and close may also be
used in synchronous helpers with owned parameters. Numeric IPv4/IPv6 addresses
and ports 1–65535 follow the exchange API. Backlog is 1–1024. Read returns at
most `min(maxBytes, 8192)` available bytes; an empty string means EOF. `maxBytes`
must be positive. Write handles partial writes/backpressure until the full binary
string is sent, then returns its length. Deadline expiry fails the operation,
including after a partial write; retrying must account for that partial delivery.
Negative timeouts fail; zero is an already-expired socket deadline.

Operations require an owned local without consuming it. One read (or listener
accept) and one write can be pending per resource; another operation in the same
direction fails with EBUSY. Read and write can run concurrently. Half-close with
a pending writer fails with EBUSY. Closing a resource fails pending operations
with ECANCELED, removes their poll registrations before descriptor reuse, and
closes exactly once. Other operation errors do not implicitly close an owner;
use TaskResult to recover, then close/drop it or attempt another operation.

Resources are rooted in their owning task scope and close after its children
join. Generated async constructors adopt transferred resources, including
resources inside owned TaskResult payloads; successful owned returns promote
them to the parent. Lazy capture cancellation, unclaimed task results, explicit
drop and failed task cleanup release their resources. Native resource cleanup
is distinct from user-defined PHP destructors, which remain outside this API.

See `examples/async/stream.php` for a finite listener/client round trip. Its
matching source test selects an available loopback port. The native lifecycle
suite asserts leases, descriptor closure, unclaimed results, owned outcome drop
and cancelled lazy extraction at O0/O3 with GC stress and sanitizers.

## Recoverable task results

`taskResultAsync($task)` consumes an unclaimed task handle and returns a lazy
`Task<owned<TaskResult<T>>>`. Construct this boundary before the operand first
runs: a failure that already reached the parent cannot be retroactively cleared.
The wrapper owns/joins the operand; its failure is captured after descendant
cleanup instead of failing siblings. Spawn the wrapper for concurrent work:

```php
$task = spawn taskResultAsync(tcpConnectAsync("127.0.0.1", 8765));
$result = await $task;
if (resultOk($result)) {
    $stream = await resultValueAsync($result);
    // use $stream, then closeSocket($stream)
} else {
    echo resultError($result);
    drop($result);
}
```

`resultOk`, `resultStatus` and `resultError` inspect without consuming the owned
outcome. Status is 0 (OK), 1 (cancelled) or 2 (error). Error is zero on success,
positive errno on failure, ECANCELED for cancellation. Inspectors work in sync
helpers too. `resultValueAsync` consumes the outcome and produces `Task<T>`:
success transfers the value once, including owned resources/objects or void;
failure propagates the stored error through ordinary structured task failure.
It can itself be wrapped to recover that failure. Dropping an outcome releases
an owned resource payload. Ownership qualifiers inside TaskResult annotations
must be preserved through locals, arguments and returns. There is no automatic
conversion to mixed, PHP exception, catch/finally or user destructor semantics.
See `examples/async/results.php`.
