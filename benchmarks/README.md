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
| `strings_snapshots.php` | 50,000 appends retaining ten string snapshots |
| `objects.php` | 2,000,000 object allocations, property reads and method calls |
| `object_methods.php` | 2,000,000 mutating method calls on one reused object |
| `objects_retained.php` | Retain 100,000 objects through GC, then read their properties |

## Current reference results

After the object and heap optimization: Apple M3 Max, PHP 8.4.25 CLI, OPcache CLI off /
JIT disabled, PXR `-O3`, median of nine after two warmups on 2026-10-16:

| Workload | PXR median | PHP median | PXR relative to PHP |
| --- | ---: | ---: | --- |
| Startup / print | 2.439 ms | 29.073 ms | 11.92× faster |
| Recursive fib(30) | 4.449 ms | 88.389 ms | 19.87× faster |
| Integer loop (5M) | 2.262 ms | 81.774 ms | 36.15× faster |
| Integer-array append + reads (10k) | 2.867 ms | 29.285 ms | 10.21× faster |
| String-key array writes + reads (5k) | 4.223 ms | 29.380 ms | 6.96× faster |
| Read-only array copies (100k × 1k items) | 7.810 ms | 30.433 ms | 3.90× faster |
| Array copy + two overwrites (10k × 1k items) | 16.839 ms | 40.515 ms | 2.41× faster |
| String append (50k) | 3.373 ms | 30.007 ms | 8.90× faster |
| String append with ten snapshots (50k) | 3.135 ms | 29.813 ms | 9.51× faster |
| Objects + methods (2M) | 100.490 ms | 188.720 ms | 1.88× faster |
| Methods on one reused object (2M) | 7.392 ms | 87.851 ms | 11.89× faster |
| Retained objects + reads (100k) | 12.121 ms | 40.530 ms | 3.34× faster |

See the [optimization report and before/after samples](results/2026-10-16-objects.md).
The [original M7 comparison](results/2026-10-16-m7.md) remains available for history.

## Limits

These are microbenchmarks of the shared PHP/PXR subset, not application or HTTP
throughput measurements. Small programs can be dominated by process startup;
optimizers can simplify scalar loops aggressively. The comparison uses PHP's
installed settings, not PHP JIT or a persistent server. Do not extrapolate the
scalar speedups to arrays, strings, objects or general PHP applications.
