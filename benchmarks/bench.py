#!/usr/bin/env python3
"""PXR benchmark harness.

Builds each benchmark with `pxr build --opt 3`, runs it, and (when the
traditional PHP interpreter is available) runs the same source with `php` so the
numbers can be compared directly. Reports the best of N runs, because wall-clock
noise on a laptop is significant.

Usage:  python3 benchmarks/bench.py [--repeat 5]
"""
from __future__ import annotations

import argparse
import os
import shutil
import subprocess
import sys
import tempfile
import time

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
PXR = os.path.join(ROOT, "target", "release", "pxr")
BENCH_DIR = os.path.join(ROOT, "benchmarks")
BENCHMARKS = ["fib.php", "loop.php"]


def find(name: str) -> str | None:
    return shutil.which(name)


def timed(cmd: list[str], repeat: int) -> tuple[float | None, str]:
    best = None
    for _ in range(repeat):
        t0 = time.perf_counter()
        p = subprocess.run(cmd, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
        dt = time.perf_counter() - t0
        if p.returncode != 0:
            return None, p.stderr.decode(errors="replace")
        best = dt if best is None else min(best, dt)
    return best, ""


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--repeat", type=int, default=3)
    ap.add_argument("--opt", type=int, default=3)
    args = ap.parse_args()

    if not os.path.exists(PXR):
        print(f"error: {PXR} not found; run `cargo build --release` first")
        return 2

    php = find("php")
    results = []

    with tempfile.TemporaryDirectory(prefix="pxr-bench-") as tmp:
        for bench in BENCHMARKS:
            src = os.path.join(BENCH_DIR, bench)
            exe = os.path.join(tmp, bench.replace(".php", ""))
            build = subprocess.run(
                [PXR, "build", src, "-o", exe, "--opt", str(args.opt)],
                stdout=subprocess.PIPE,
                stderr=subprocess.PIPE,
            )
            if build.returncode != 0:
                print(f"build failed for {bench}:\n{build.stderr.decode()}")
                return 1

            pxr_time, err = timed([exe], args.repeat)
            if pxr_time is None:
                print(f"pxr run failed for {bench}: {err}")
                return 1
            row = {"bench": bench, "pxr": pxr_time, "php": None}
            if php:
                php_time, err = timed([php, src], args.repeat)
                row["php"] = php_time
            results.append(row)

    print(f"{'benchmark':<12}{'PXR (ms)':>12}{'PHP (ms)':>12}{'speedup':>12}")
    print("-" * 48)
    for r in results:
        pxr_ms = r["pxr"] * 1e3
        if r["php"]:
            php_ms = r["php"] * 1e3
            print(f"{r['bench']:<12}{pxr_ms:>12.1f}{php_ms:>12.1f}{r['php'] / r['pxr']:>11.1f}x")
        else:
            print(f"{r['bench']:<12}{pxr_ms:>12.1f}{'n/a':>12}{'n/a':>12}")
    if not php:
        print("\nnote: `php` not found; install PHP to compare against the interpreter")
    return 0


if __name__ == "__main__":
    sys.exit(main())
