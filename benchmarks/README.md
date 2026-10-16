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
two warmups and nine measured runs. It alternates PXR/PHP execution order, checks
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
| `array_copy.php` | 100,000 read-only copies of a 1,000-element array |
| `strings.php` | 50,000 appends, producing a 400,000-byte string |
| `objects.php` | 2,000,000 object allocations, property reads and method calls |

## Current reference results

Apple M3 Max, PHP 8.4.25 CLI, OPcache CLI off / JIT disabled, PXR `-O3`, median of
nine after two warmups, measured 2026-10-16:

| Workload | PXR median | PHP median | PXR relative to PHP |
| --- | ---: | ---: | --- |
| Startup / print | 2.324 ms | 28.323 ms | 12.19× faster |
| Recursive fib(30) | 4.211 ms | 86.670 ms | 20.58× faster |
| Integer loop (5M) | 2.117 ms | 80.486 ms | 38.03× faster |
| Array append + reads (10k) | 83.585 ms | 28.370 ms | 2.95× slower |
| Array copies (100k × 1k items) | 255.348 ms | 29.761 ms | 8.58× slower |
| String append (50k) | 186.140 ms | 29.697 ms | 6.27× slower |
| Objects + methods (2M) | 284.242 ms | 188.742 ms | 1.51× slower |

See the [full report](results/2026-10-16-m7.md) and
[raw samples](results/2026-10-16-m7.json). The full report explains startup costs,
configuration and current implementation bottlenecks.

## Limits

These are microbenchmarks of the shared PHP/PXR subset, not application or HTTP
throughput measurements. Small programs can be dominated by process startup;
optimizers can simplify scalar loops aggressively. The comparison uses PHP's
installed settings, not PHP JIT or a persistent server. Do not extrapolate the
scalar speedups to arrays, strings, objects or general PHP applications.
