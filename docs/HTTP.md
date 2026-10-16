# Native HTTP and PHP-style request data — M7 in progress

PXR has a bounded HTTP/1.x server adapter with isolated PHP-style request globals,
buffered output, response headers and status codes. It runs on the native
single-thread executor without PHP. This is an implemented subset, not a full
PHP web SAPI or a complete HTTP/1.1 implementation.

```php
<?php
async function handle(): void {
    header("Content-Type: text/plain; charset=UTF-8");
    if ($_SERVER["PATH_INFO"] === "/hello") {
        $name = strval($_GET["name"] ?? "world");
        await sleepAsync(0);
        echo "Hello, ", $name, "!\n";
        return;
    }
    http_response_code(404);
    echo "Not found\n";
}

async function main(): void {
    await httpServeAsync("127.0.0.1", 8080, "handle", 0, 1048576);
}
```

Run `pxr run examples/async/http.php`, then visit
`http://127.0.0.1:8080/hello?name=Ada` or use curl. The server runs until cancelled
or the process stops. `examples/async/http_demo.php` is a finite demonstration
with a native server and two native clients on loopback port 8767; it prints the
complete responses and exits. No Python server is needed.

## Server and handler contract

`httpServeAsync(string address, int port, literal string handler, int maxRequests,
int maxBytes, int timeoutMs = 30000, int maxConcurrent = 64)` creates a lazy
`Task<void>`. The handler names a native, non-generic, free
`async function(): void` or `async function(): string`. A returned string is
appended after buffered output. Names follow the same literal/full-name rules as
[`tcpServeAsync`](ASYNC_SERVER.md); there is no dynamic callback dispatch.

Numeric IPv4/IPv6 addressing, ports, lazy/spawned task ownership, finite request
counts, concurrency limits, cancellation, and acceptance-to-response deadlines
follow the TCP server contract. IPv6 uses a separate IPv6-only listener.
A finite request count includes connections rejected with protocol errors.
At capacity the listener suspends until a connection releases its slot.
Request/response objects, router classes, async methods and streaming socket
handles are still pending. Ordinary `if`/`match` routing on request data works.

## PHP-style request globals

These names are visible without a `global` declaration in native free functions
and methods, including synchronous helpers:

| Global | Implemented data |
| --- | --- |
| `$_GET` | Query-string parameters for any HTTP method. |
| `$_POST` | POST bodies with `application/x-www-form-urlencoded` media type. |
| `$_COOKIE` | Request cookies, including percent-decoded values. |
| `$_SERVER` | Request method/URI/protocol, path/query, local/peer address and port, and header fields described below. |
| `$_REQUEST` | An independent recursive GET-then-POST merge; POST wins conflicts, cookies are excluded. |
| `$_FILES` | Empty; multipart uploads are rejected with 415 before invoking a handler. |

Each is typed `array<mixed>`. Reads, ordinary copies and element assignments use
existing PHP array semantics. Use `??` for possibly absent keys and `strval` or
a type guard when a concrete type is required. The existing PXR language/builtin
subset still applies; `isset`, `empty` and `unset` are not added by this API.
Nested parsed arrays can be read
through existing `is_array` guards. The compiler's existing restriction on nested
array assignment still applies. Replacing an entire request-global binding,
shadowing it with a parameter/foreach destination, moving it as an owner, and
storing task handles in it are rejected. `$_SESSION`, `$_ENV` and `$GLOBALS` are
not implemented and receive diagnostics; this does not add sessions.

Query/form parsing supports percent decoding, `+` as space, scalar duplicates
(last wins), canonical integer keys, `[]` append and balanced named bracket
paths. Top-level spaces/dots are normalized to underscores, leading spaces are
ignored, and names end at an embedded NUL, matching common PHP input handling.
Cookie names are not URL-decoded, cookie `+` remains literal, and the first value
for a duplicate leaf wins. Normalization cannot forge `__Host-`/`__Secure-`
cookie prefixes. Parameter parsing allows at most 1,024 nonempty fields per
source and 16 bracket levels. Malformed/trailing bracket syntax or exceeding
those limits returns 400; PHP's more permissive malformed-name handling and
php.ini configuration are not reproduced. No input is silently truncated.

`$_REQUEST` is a snapshot built before the handler. Later writes to `$_GET` or
`$_POST` do not change it. Request data, headers, status and output belong to the
accepted connection. Async descendants inherit that request context, which
remains alive until they finish. Different requests never share a context, even
when one awaits while another runs. Tasks outside HTTP use independent,
initially empty CLI request arrays; those arrays do not initialize HTTP requests.
PHP-worker globals are separate and are not synchronized with native globals.

`$_SERVER` supplies `REQUEST_METHOD`, `REQUEST_URI`, `SERVER_PROTOCOL`,
`QUERY_STRING`, `PATH_INFO`, `SERVER_SOFTWARE`, and available `SERVER_ADDR`,
`SERVER_PORT`, `REMOTE_ADDR`, `REMOTE_PORT`. Address/port values are strings.
`PATH_INFO` is the undecoded origin-form path in this adapter, rather than
web-server script/path-info dispatch. Header names are mapped to uppercase
`HTTP_*` keys with hyphens converted to underscores. Content-Type and
Content-Length use `CONTENT_TYPE` and `CONTENT_LENGTH` instead. Repeated ordinary
headers are comma-joined; cookies use semicolons. `HTTP_PROXY` is excluded.
Script filesystem variables, authentication/session state, HTTPS and proxy trust
configuration are not synthesized. `httpRequestBody(): string` returns the raw,
binary-safe body; unknown media types leave `$_POST` empty. Multipart uploads
and non-identity Content-Encoding receive 415.

## Output, headers and status

`echo`, `print`, and `printf` inside a handler or its native helpers/children append
to that request's bounded buffer. Output outside an HTTP task keeps its ordinary
stdout behavior. Handler completion first joins its descendants, then combines
buffered output and any returned string into one response. Headers can still be
changed after buffered output because nothing has been committed to the socket.
There is no streaming/flush API. PHP-worker output is not captured by this native
adapter; interop still blocks the executor and is not a request-aware PHP SAPI.

* `header(string line, bool replace = true, int responseCode = 0): void` stores a
  nonempty header. Replacing is case-insensitive; `false` preserves repeated
  headers, including Set-Cookie. HTTP/1.0 or HTTP/1.1 status lines set the code;
  their custom reason text is not retained. Location defaults to 302 unless a
  201 or 3xx status is already selected.
* `http_response_code(int code = 0): int` returns the previous/current code and
  optionally sets a final status from 200–599. The initial code is 200.
* `httpRequestBody(): string` returns the framed raw body.

These are reserved native intrinsics, including namespace fallback and function
imports. The optional arguments belong to these APIs and do not add general
user-defined default parameters. They require an active native HTTP request at
runtime; using them outside one reports a clear runtime error.

The adapter owns Content-Length and Connection framing and rejects overrides of
Content-Length, Transfer-Encoding, Connection, Trailer and Upgrade. Header names
must be HTTP tokens; values cannot contain CR/LF, NUL or disallowed controls.
Invalid response metadata or excessive output produces a per-connection 500
response after the handler finishes, with untrusted headers/output discarded.
It does not interrupt arbitrary synchronous handler code. The default content
type is `text/html; charset=UTF-8`; Date is generated unless supplied. A HEAD
request invokes the handler and advertises the computed representation length
without a payload. Statuses 204/304 omit payload and Content-Length; 205 has
Content-Length zero. No `setcookie`, session, output-buffer control or complete
header-management API is introduced.

## HTTP framing and limits

The adapter accepts strict CRLF-delimited HTTP/1.0 and HTTP/1.1 request lines and
origin-form targets. It serves one request per connection with Content-Length
and `Connection: close`. Unlike the raw TCP server, it invokes the handler as
soon as the headers and declared body are complete, without waiting for EOF.
Missing Content-Length means no body. HTTP/1.1 requires one valid Host field.
Malformed header tokens/values, whitespace before a colon, folded headers,
conflicting/duplicate Content-Length and TE+CL are rejected with 400.

Headers, including their terminator, are limited to 16 KiB and 100 fields;
exceeding these limits returns 431. Targets are limited to 8 KiB. `maxBytes` is
an independent nonnegative limit on declared request body content and combined
handler output/result, excluding headers. Oversized requests receive 413;
invalid/oversized handler responses receive 500. Fixed error responses have a
small reserved body even when `maxBytes` is zero. Buffers, parsed variables,
string copies and arbitrary handler allocations also consume memory; this is
not a total process memory bound.

Transfer-Encoding (including chunked requests) and Upgrade receive 501, Expect
receives 417, and unsupported versions receive 505. Keep-alive, pipelining
execution, CONNECT/proxy/asterisk targets, trailers, compression, TLS and HTTP/2
are not implemented. Additional bytes are not dispatched as another request.
After sending the response, the server shuts down writes so the peer sees EOF,
then drains at most 64 KiB/100 ms of input before closing, bounded by the existing
connection deadline. This reduces resets when rejecting unread request bodies;
error responses remain best effort. Drain expiration completes that connection
successfully. Pending reads, handler execution and response writes still use the
single absolute connection deadline.

Protocol/input-validation errors are local HTTP responses and do not stop the
listener. General task failures, connection deadlines and socket errors retain
the current fail-fast structured task policy: cancel/join the server subtree,
close resources, report the first error, and exit 255. Recoverable task errors,
per-connection transport-error recovery and graceful shutdown with a drain
budget remain M7 work. Existing synchronous fatal errors retain their process
behavior. This slice is not yet a production web-server compatibility claim.

## Verification and references

Source acceptance tests use real clients that keep their write sides open.
They cover fragmented headers/bodies, binary 1 MiB transfers, limits, HTTP status
and header behavior, PHP-style output and globals, nested parameters, and
connection-local protocol errors followed by valid requests. A protocol gate
proves a fast request finishes while another handler is suspended; globals,
status, headers and child-task output remain isolated. The suites run at
`-O0`/`-O3`, normal/forced GC, and optional ASan/UBSan.

The native C suite additionally feeds headers byte by byte, tests truncation,
NUL/control bytes and deterministic arbitrary input, forces collection while
request/output buffers are live, verifies nested REQUEST merging, and checks
read deadlines, bounded teardown and cancellation through a handler's joining
grandchild. Cleanup and descriptor reuse are checked before process teardown.
See [ASYNC_REFERENCES.md](ASYNC_REFERENCES.md) for RFCs, PHP source/manuals and the
pinned llhttp checkout studied. No upstream parser code is vendored or copied.
