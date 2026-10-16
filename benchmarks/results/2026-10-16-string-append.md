# String append optimization — 2026-10-16

Local string accumulators now reuse geometrically growing capacity until a
snapshot is taken. The 50,000-append workload fell from 195.829 ms to 3.763 ms
(**52.04× faster** than the previous PXR runtime). A new workload that retains ten
snapshots while appending fell from 270.371 ms to 5.103 ms (**52.98× faster**).
Both workloads validate output against ordinary PHP on every run.

| Workload | PXR before | PXR after | PXR improvement | PHP after |
| --- | ---: | ---: | ---: | ---: |
| Startup / print | 2.874 ms | 2.539 ms | 1.13× | 30.470 ms |
| Recursive fib(30) | 4.993 ms | 5.091 ms | 0.98× | 92.238 ms |
| Integer loop (5M) | 2.302 ms | 2.558 ms | 0.90× | 82.295 ms |
| Integer-array append + reads (10k) | 2.808 ms | 3.240 ms | 0.87× | 29.655 ms |
| String-key array writes + reads (5k) | 4.168 ms | 4.293 ms | 0.97× | 30.530 ms |
| Read-only array copies (100k × 1k items) | 8.813 ms | 8.948 ms | 0.98× | 32.042 ms |
| Array copy + two overwrites (10k × 1k items) | 20.970 ms | 20.894 ms | 1.00× | 43.549 ms |
| String append (50k) | 195.829 ms | 3.763 ms | 52.04× | 30.765 ms |
| String append with ten snapshots (50k) | 270.371 ms | 5.103 ms | 52.98× | 32.056 ms |
| Objects + methods (2M) | 325.665 ms | 328.313 ms | 0.99× | 198.383 ms |

An improvement above 1 means PXR became faster. The string workloads are the
intended optimization. Other timings vary, particularly startup-dominated cases;
they do not establish improvements or steady-state throughput regressions. The
object workload remains slower than PHP and is the next optimization target.

## Method and reproduction

* Apple M3 Max, 36 GiB, macOS 15.8.1 arm64; Apple clang 17.0.0.
* PHP 8.4.25 CLI with installed Herd settings, CLI OPcache off and JIT disabled.
  PXR release compiler, programs built with `--opt 3`.
* Two warmups and nine measured runs per runtime. Execution order alternates
  PXR/PHP; timings include startup and teardown and exclude compilation.
* All ten source hashes and outputs agree between reports. The new snapshot
  workload also runs under the saved baseline compiler. No test suites ran
  during these two measurement runs.
* The baseline compiler is revision `c790e30`. The after report uses the modified
  compiler/runtime. Both reports record binary SHA-256 values; their git revision
  is the workspace HEAD before this optimization was committed.

```sh
# Save the baseline compiler before rebuilding the modified compiler/runtime.
cp target/release/pxr /tmp/pxr-before-string-append
cargo build --release -p pxr
python3 benchmarks/bench.py --pxr /tmp/pxr-before-string-append \
  --repeat 9 --warmup 2 --json /tmp/string-append-before.json
python3 benchmarks/bench.py --repeat 9 --warmup 2 \
  --json /tmp/string-append-after.json
```

[Before raw samples](2026-10-16-string-append-before.json),
[after raw samples](2026-10-16-string-append-after.json).
To reproduce later, build revision `c790e30` separately and select its compiler
with `--pxr`, then build this optimization revision for the second run.

PHP startup is about 30 ms and PXR startup about 2.5 ms in this environment.
The optimized string workloads approach the PXR startup floor. Their end-to-end
wins over this installed PHP CLI do not establish string throughput in a
persistent server or application-wide speedups. Historical results remain in the
[copy-on-write array report](2026-10-16-array-cow.md),
[array-index report](2026-10-16-array-index.md) and
[original M7 comparison](2026-10-16-m7.md).

## Implementation and correctness

* The backend recognizes a local string concatenation whose sole result is
  immediately stored back to its left-hand local. This includes `.=` and
  `$text = $text . $suffix`. Other concatenations remain independent strings.
* A compiler-only append intrinsic copies ordinary incoming strings into private
  capacity-backed allocations. Further appends reuse capacity; growth copies
  geometrically instead of copying the whole prefix on every iteration. The
  public `{ len, data[] }` layout remains unchanged. Frozen snapshots retain
  their reserved capacity until collection; freezing does not compact buffers.
* Snapshot barriers freeze the shared allocation before a loaded accumulator
  escapes through assignment, array values/keys, object fields, mixed boxing,
  returns, borrows or unknown calls. Later appends separate, so snapshots retain
  their bytes. Earlier SSA loads used after a write and cross-block loads also
  freeze. Only explicitly known non-retaining readers avoid this barrier.
* The pass runs before async frame lowering and GC root calculation. Frame cells
  preserve the same values across suspension, including awaiting the right-hand
  operand of an append. Old growth buffers remain managed until GC reclaims them.
* Native tests cover binary data, self-append, zero-length suffixes, capacity reuse,
  frozen snapshots, bounded growth count, collection during appending, and checked
  overflow. Compiler tests cover escaping values, unknown calls, old SSA loads,
  cross-block values and concat results with extra aliases. Source tests compare
  PHP behavior and exercise returns, task capture, borrows, snapshots and GC stress.

Retaining a snapshot before every append still requires copying on the next
write. Mixed accumulators, property targets and array-element targets retain the
ordinary concatenation path. This optimization introduces no syntax restrictions.

Validation: 272 workspace tests and 70 ASan/UBSan checks passed, plus the
native ThreadSanitizer suite and four worker source tests under ThreadSanitizer.
The release build, checks for every example and running every example also passed.
