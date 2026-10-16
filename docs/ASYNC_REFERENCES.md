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
task types and consumption checker enforce the first source slice of this runtime contract;
these references do not establish PHP compatibility or source support.

See [ASYNC_RUNTIME.md](ASYNC_RUNTIME.md) for the implemented internal contract and
[ASYNC.md](ASYNC.md) for supported source syntax and remaining work.

For the TCP exchange slice, also studied `src/unix/stream.c` connect completion
and `src/unix/tcp.c` in the pinned libuv checkout. Primary platform documentation
was checked for [nonblocking connect and SO_ERROR](https://man7.org/linux/man-pages/man2/connect.2.html),
[send flags and SIGPIPE](https://man7.org/linux/man-pages/man2/send.2.html), and
[Apple socket options](https://developer.apple.com/library/archive/documentation/System/Conceptual/ManPages_iPhoneOS/man2/setsockopt.2.html).
The PXR operation uses the platform's MSG_NOSIGNAL or SO_NOSIGPIPE facility; it
does not install a process-wide signal handler. The implementation remains
original and adds no upstream build dependency.


For the native server slice, also studied `uv__server_io`/`uv_accept` in the pinned
libuv `src/unix/stream.c`: pause acceptance at capacity and explicitly own accepted
descriptors. Checked [accept semantics and flag inheritance](https://man7.org/linux/man-pages/man2/accept.2.html),
[listen backlog limits](https://man7.org/linux/man-pages/man2/listen.2.html), and
[IPV6_V6ONLY](https://man7.org/linux/man-pages/man7/ipv6.7.html). PXR configures
accepted descriptors itself and requests separate IPv4/IPv6 listeners. The
server implementation remains original; no upstream code or dependency was added.


## HTTP adapter references

Read on 2026-10-16. Also cloned [nodejs/llhttp](https://github.com/nodejs/llhttp)
with `git clone --depth 1` to the same external reference directory, revision
`69936dbc22922e596a05dfa912fc11d9a7af2136`. Read its MIT `LICENSE`, strict-parser
flags in `README.md`, and Content-Length/Transfer-Encoding/chunk states in
`src/llhttp/http.ts`. These informed framing ambiguity checks and malformed-input
tests. PXR uses its own bounded parser; llhttp is not a runtime/build dependency
and no llhttp source was copied.

Primary protocol reading: [RFC 9112](https://www.rfc-editor.org/rfc/rfc9112.html)
(request line, field syntax, message length, incomplete messages and teardown)
and [RFC 9110](https://www.rfc-editor.org/rfc/rfc9110.html) (Host, HEAD, status and
response metadata). For chunked input, additionally read RFC 9112 §7.1–7.1.3
(chunk framing, extensions and trailers), RFC 9110 §6.5 (trailer semantics) and
§10.1.1 (100-continue). These informed incremental decoding, separate metadata
limits, discarded nonessential trailers and nonblocking interim responses.
The subset still rejects Upgrade and other documented unsupported forms;
studying the RFCs is not a conformance claim.

PHP syntax/semantics checked against the official manuals for
[superglobals](https://www.php.net/manual/en/language.variables.superglobals.php),
[GET](https://www.php.net/manual/en/reserved.variables.get.php),
[POST](https://www.php.net/manual/en/reserved.variables.post.php),
[COOKIE](https://www.php.net/manual/en/reserved.variables.cookies.php),
[REQUEST](https://www.php.net/manual/en/reserved.variables.request.php),
[header](https://www.php.net/manual/en/function.header.php), and
[http_response_code](https://www.php.net/manual/en/function.http-response-code.php).
Also studied input-name normalization, bracket/cookie parsing and recursive
REQUEST merging in the existing ignored PHP 8.5.11 source archive,
`references/php-src-php-8.5.11/main/php_variables.c` (SHA-256
`a2d15a0a9505cf17e8b45dc29220368b0efb96e31523e40d63849aadde384f9b`;
version verified in `main/php_version.h`; PHP License 3.01 read). No PHP source
was copied. [HTTP.md](HTTP.md) records fixed GP precedence, input limits,
malformed-name handling and other deliberate compatibility boundaries.

## Threaded workers and channels

Read on 2026-10-16: the previously pinned libuv `src/threadpool.c`, especially
worker completion publication, loop notification, cancellation and lock ordering.
The original PXR implementation uses separate per-worker heaps/executors and
bounded serialized duplex queues, rather than copying libuv's pool. No upstream
source was copied and no new dependency was added.

Checked primary POSIX references for [thread creation](https://pubs.opengroup.org/onlinepubs/000095399/functions/pthread_create.html),
[mutex ownership](https://pubs.opengroup.org/onlinepubs/9799919799/functions/pthread_mutex_lock.html),
and [memory synchronization](https://pubs.opengroup.org/onlinepubs/9799919799/basedefs/V1_chap04.html).
These informed joinable worker lifetimes, mutex-protected queue/reference state,
and atomic publication after cleanup. Cancellation uses an owned pipe and
cooperative task joins; no pthread cancellation of arbitrary generated code.
[PARALLEL.md](PARALLEL.md) specifies PXR's transfer and cancellation contract.
