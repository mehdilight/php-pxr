# PXR benchmarks

Compare native PXR programs against the ordinary installed PHP CLI using exactly
the same PHP source. The suite includes slower cases as well as scalar wins.

## Running

```sh
./benchmarks/run.sh
python3 benchmarks/bench.py --repeat 9 --warmup 2 --opt 3 \
  --json /tmp/pxr-benchmark.json
```

The shell helper builds the release compiler first. By default the harness uses
`target/release/pxr`, the `php` executable on PATH with its current configuration,
two warmups and nine measured runs. `--pxr /path/to/pxr` selects a saved compiler
for before/after comparisons. It alternates PXR/PHP execution order, checks
identical output on every execution, and reports median process wall time.
Nonzero exits, stderr, mismatched results and execution timeouts fail the run.
Compilation is excluded from timings; startup and teardown are included. The
optional JSON report stores all samples, best/median/max, build times, source
hashes and environment/PHP configuration metadata. Without PHP, it reports PXR
only and cannot provide an interpreter comparison.

## Workloads

| File | What it measures |
| --- | --- |
| `startup.php` | Minimal process startup and printing |
| `fib.php` | Naive recursive `fib(30)`, call-heavy integer code |
| `loop.php` | 5,000,000-iteration integer accumulation loop |
| `arrays.php` | Append 10,000 integers, then read each index |
| `array_keys.php` | Build and look up 5,000 dynamic string keys |
| `array_copy.php` | 100,000 read-only copies of a 1,000-element array |
| `strings.php` | 50,000 appends, producing a 400,000-byte string |
| `objects.php` | 2,000,000 object allocations, property reads and method calls |

## Current reference results

After the first array optimization: Apple M3 Max, PHP 8.4.25 CLI, OPcache CLI off /
JIT disabled, PXR `-O3`, median of nine after two warmups on 2026-10-16:

| Workload | PXR median | PHP median | PXR relative to PHP |
| --- | ---: | ---: | --- |
| Startup / print | 2.461 ms | 29.153 ms | 11.85× faster |
| Recursive fib(30) | 4.425 ms | 87.536 ms | 19.78× faster |
| Integer loop (5M) | 2.212 ms | 81.419 ms | 36.81× faster |
| Integer-array append + reads (10k) | 2.991 ms | 28.795 ms | 9.63× faster |
| String-key array writes + reads (5k) | 4.323 ms | 29.344 ms | 6.79× faster |
| Read-only array copies (100k × 1k items) | 256.500 ms | 30.538 ms | 8.40× slower |
| String append (50k) | 187.133 ms | 29.931 ms | 6.25× slower |
| Objects + methods (2M) | 282.038 ms | 187.631 ms | 1.50× slower |

See the [optimization report and before/after samples](results/2026-10-16-array-index.md).
The [original M7 comparison](results/2026-10-16-m7.md) remains available for history.

## Limits

These are microbenchmarks of the shared PHP/PXR subset, not application or HTTP
throughput measurements. Small programs can be dominated by process startup;
optimizers can simplify scalar loops aggressively. The comparison uses PHP's
installed settings, not PHP JIT or a persistent server. Do not extrapolate the
scalar speedups to arrays, strings, objects or general PHP applications.
