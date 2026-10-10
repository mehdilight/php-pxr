# PXR benchmarks

A tiny, honest benchmark harness that compares compiled PXR binaries against the
traditional PHP interpreter on programs that are **valid PHP and valid PXR**.

## Running

```sh
./benchmarks/run.sh            # builds pxr --release, then runs the harness
python3 benchmarks/bench.py --repeat 5 --opt 3
```

The harness:

1. builds each `benchmarks/*.php` with `pxr build --opt 3`,
2. runs the resulting native executable and the same source under `php`,
3. reports the **best of N** wall‑clock runs for each.

## Benchmarks

| File | What it measures |
| --- | --- |
| `fib.php` | naive recursive `fib(30)` — call‑heavy integer workload |
| `loop.php` | a 5,000,000‑iteration integer accumulation loop |

## Reference results

Measured on an Apple‑silicon laptop, best of 3, `--opt 3` (your numbers will
differ):

| benchmark | PXR (ms) | PHP (ms) | speedup |
| --- | ---: | ---: | ---: |
| `fib.php` | 7.5 | 87.8 | 11.7× |
| `loop.php` | 2.4 | 77.9 | 33.0× |

## Caveats

* These are microbenchmarks of the *shared, statically typed subset*. They are
  not a claim that PXR is universally faster than PHP, and they do not exercise
  features PXR does not yet have (arrays, objects, strings in loops).
* Best‑of‑N reduces (but does not remove) scheduler and thermal noise.
* Compilation time is excluded from the measurements; only program execution is
  timed.
