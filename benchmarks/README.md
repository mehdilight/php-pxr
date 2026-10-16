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
| `array_copy_write.php` | 10,000 copies of a 1,000-element array, with two overwrites per copy |
| `strings.php` | 50,000 appends, producing a 400,000-byte string |
| `objects.php` | 2,000,000 object allocations, property reads and method calls |

## Current reference results

After the copy-on-write array optimization: Apple M3 Max, PHP 8.4.25 CLI, OPcache CLI off /
JIT disabled, PXR `-O3`, median of nine after two warmups on 2026-10-16:

| Workload | PXR median | PHP median | PXR relative to PHP |
| --- | ---: | ---: | --- |
| Startup / print | 2.442 ms | 29.893 ms | 12.24× faster |
| Recursive fib(30) | 4.468 ms | 89.097 ms | 19.94× faster |
| Integer loop (5M) | 2.317 ms | 82.416 ms | 35.57× faster |
| Integer-array append + reads (10k) | 3.112 ms | 29.339 ms | 9.43× faster |
| String-key array writes + reads (5k) | 4.060 ms | 29.574 ms | 7.28× faster |
| Read-only array copies (100k × 1k items) | 8.210 ms | 30.405 ms | 3.70× faster |
| Array copy + two overwrites (10k × 1k items) | 16.834 ms | 40.867 ms | 2.43× faster |
| String append (50k) | 187.149 ms | 30.197 ms | 6.20× slower |
| Objects + methods (2M) | 290.750 ms | 189.499 ms | 1.53× slower |

See the [optimization report and before/after samples](results/2026-10-16-array-cow.md).
The [original M7 comparison](results/2026-10-16-m7.md) remains available for history.

## Limits

These are microbenchmarks of the shared PHP/PXR subset, not application or HTTP
throughput measurements. Small programs can be dominated by process startup;
optimizers can simplify scalar loops aggressively. The comparison uses PHP's
installed settings, not PHP JIT or a persistent server. Do not extrapolate the
scalar speedups to arrays, strings, objects or general PHP applications.
