# Async TCP exchange and cancellation

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
is a request-then-response operation, not a general full-duplex stream API.

Addresses must be numeric IPv4 or IPv6 literals; pass `"::1"` without URL brackets
for IPv6 loopback. Hostnames, scoped IPv6 addresses, URLs, TLS, DNS, listening,
accepted socket handles, persistent connections and streaming read/write APIs
are not supplied by this slice. Invalid addresses and embedded NULs fail with
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

`sleepAsync`, `tcpExchangeAsync` and `cancelTask` are case-insensitive reserved
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
