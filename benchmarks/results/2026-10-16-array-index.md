# First optimization: indexed arrays — 2026-10-16

Dense integer arrays now address entries directly. Other keys use a hash index
while the entry buffer retains PHP insertion order. Appending uses a cached next
integer key instead of scanning existing entries. Array copies remain eager.

The 10,000-element integer-array benchmark improved from 86.515 ms to 2.991 ms
(**28.93×**). A new string-key benchmark improved from 45.434 ms to 4.323 ms
(**10.51×**). These compare PXR against its previous runtime, with identical source.

| Workload | PXR before | PXR after | PXR improvement | PHP after |
| --- | ---: | ---: | ---: | ---: |
| Startup / print | 2.470 ms | 2.461 ms | 1.00× | 29.153 ms |
| Recursive fib(30) | 4.629 ms | 4.425 ms | 1.05× | 87.536 ms |
| Integer loop (5M) | 2.257 ms | 2.212 ms | 1.02× | 81.419 ms |
| Integer-array append + reads (10k) | 86.515 ms | 2.991 ms | 28.93× | 28.795 ms |
| String-key array writes + reads (5k) | 45.434 ms | 4.323 ms | 10.51× | 29.344 ms |
| Read-only array copies (100k × 1k items) | 258.379 ms | 256.500 ms | 1.01× | 30.538 ms |
| String append (50k) | 188.857 ms | 187.133 ms | 1.01× | 29.931 ms |
| Objects + methods (2M) | 281.672 ms | 282.038 ms | 1.00× | 187.631 ms |

An improvement ratio above 1 means the new PXR run was faster. Small differences
outside the two lookup workloads should be treated as run-to-run noise, not
attributed to this change. Copying, string append and object costs remain future
optimization work; the table deliberately includes their slower results.

## Method and reproduction

* Apple M3 Max, 36 GiB, macOS 15.8.1 arm64; Apple clang 17.0.0.
* PHP 8.4.25 CLI with its installed Herd configuration; CLI OPcache off and JIT
  disabled. PXR release compiler, programs built with `--opt 3`.
* Two warmups, nine measured runs per runtime, alternating PXR/PHP order within
  each benchmark. Medians include process startup/teardown; compilation is excluded.
* Both runs validate every output against the same PHP source, not just timings.
  The newly added `array_keys.php` runs under both the saved old compiler and new
  compiler. All eight source hashes and outputs agree between reports.
* The baseline compiler was copied from the `2fb44a4` build before changes. The
  second run uses the modified runtime. Compiler SHA-256 values in the raw reports
  identify the exact binaries; the recorded git revision is the workspace HEAD
  during measurement, before the optimization commit.

```sh
# Save the old release compiler BEFORE rebuilding the modified runtime.
cp target/release/pxr /tmp/pxr-before-array-index
cargo build --release -p pxr
python3 benchmarks/bench.py --pxr /tmp/pxr-before-array-index \
  --repeat 9 --warmup 2 --json /tmp/array-index-before.json
python3 benchmarks/bench.py --repeat 9 --warmup 2 \
  --json /tmp/array-index-after.json
```

[Before raw samples](2026-10-16-array-index-before.json),
[after raw samples](2026-10-16-array-index-after.json).

The old compiler copy is a local measurement artifact, not a repository binary.
To reproduce later, build revision `2fb44a4` separately and pass its release binary
using `--pxr`; build the optimization revision for the second run.

PHP's measured startup is about 29 ms and PXR's about 2.5 ms. The optimized array
programs are now close to that PXR startup floor. Their end-to-end wins against
this PHP CLI do not establish better array throughput in a long-running server.
The saved original [M7 comparison](2026-10-16-m7.md) remains historical evidence.

## Correctness and implementation

* Dense integer keys `0..n-1` keep a direct-address fast path with no hash allocation.
* Sparse integer/string keys use an open-addressed position index kept at most
  half full. Expected lookup/insertion is constant time for fixed-length keys;
  collision chains remain correct. Rebuilds preserve insertion order.
* Heap tracing roots the index allocation through its array header. Copies have
  independent entries and rebuild a non-packed index lazily on first key lookup.
* Cached append metadata survives copying and transition from a packed list.
  PHP 8.3+ negative-key progression is preserved, `"-0"` stays a string key, and
  appending after INT64_MAX fails instead of overflowing. These also correct
  existing boundary mismatches in the old scanning implementation.
* Native tests cover deterministic mixed-key overwrites/misses, deliberate hash
  collisions, insertion order, GC during growth, copied snapshots and key bounds.
  Source tests compare behavior with installed PHP.

Full workspace and ASan/UBSan checks cover existing PHP interop, HTTP, ownership,
async frames and worker/channel array transfer as well as the new native tests.
The native thread suite also runs under ThreadSanitizer.

Validation: 264 workspace tests, 68 ASan/UBSan checks, the native
ThreadSanitizer suite, release build and example checks passed.
