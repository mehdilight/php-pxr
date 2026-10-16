#!/usr/bin/env python3
"""Compare identical PHP/PXR programs, validating output on every execution.

Reports median and best process wall time, excludes compilation, warms both
runtimes and alternates execution order. --json preserves samples and metadata.
"""
from __future__ import annotations

import argparse
import datetime
import hashlib
import json
import os
from pathlib import Path
import platform
import shutil
import statistics
import subprocess
import sys
import tempfile
import time

ROOT = Path(__file__).resolve().parent.parent
PXR = ROOT / "target" / "release" / "pxr"
BENCH_DIR = ROOT / "benchmarks"
BENCHMARKS = ["startup.php", "fib.php", "loop.php", "arrays.php", "array_keys.php",
              "array_copy.php", "strings.php", "objects.php"]


def run(cmd: list[str], env: dict[str, str] | None = None) -> tuple[float, bytes]:
    start = time.perf_counter_ns()
    result = subprocess.run(cmd, capture_output=True, env=env, timeout=60)
    elapsed = (time.perf_counter_ns() - start) / 1e9
    if result.returncode or result.stderr:
        raise RuntimeError(f"{cmd!r}: exit={result.returncode}\n"
                           f"{result.stderr.decode(errors='replace')}")
    return elapsed, result.stdout


def capture(cmd: list[str]) -> str:
    return subprocess.check_output(cmd, text=True, timeout=15).strip()


def metadata(php: str | None) -> dict:
    details = {
        "measured_at_utc": datetime.datetime.now(datetime.timezone.utc).isoformat(),
        "platform": platform.platform(), "machine": platform.machine(),
        "compiler_revision": capture(["git", "rev-parse", "HEAD"]),
        "working_tree_dirty": bool(capture(["git", "status", "--porcelain"])),
        "pxr_binary_path": str(PXR),
        "pxr_binary_sha256": hashlib.sha256(PXR.read_bytes()).hexdigest(),
        "pxr_version": capture([str(PXR), "--version"]),
        "clang_version": capture([os.environ.get("CC", "clang"), "--version"]),
        "php_command": [php] if php else None,
        "php_version": capture([php, "-v"]) if php else None,
    }
    if platform.system() == "Darwin":
        details["cpu"] = capture(["sysctl", "-n", "machdep.cpu.brand_string"])
        details["memory_bytes"] = int(capture(["sysctl", "-n", "hw.memsize"]))
    else:
        details["cpu"] = platform.processor()
    if php:
        details["php_settings"] = json.loads(capture([php, "-r", '''
            echo json_encode([
                "loaded_ini" => php_ini_loaded_file(),
                "scanned_ini" => php_ini_scanned_files(),
                "opcache.enable_cli" => ini_get("opcache.enable_cli"),
                "opcache.jit" => ini_get("opcache.jit"),
                "opcache.jit_buffer_size" => ini_get("opcache.jit_buffer_size")
            ]);
        ''']))
    return details


def summarize(samples: list[float]) -> dict:
    return {"samples_seconds": samples, "median_seconds": statistics.median(samples),
            "best_seconds": min(samples), "max_seconds": max(samples)}


def main() -> int:
    global PXR
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--pxr", type=Path, default=PXR, help="release compiler to measure")
    parser.add_argument("--repeat", type=int, default=9)
    parser.add_argument("--warmup", type=int, default=2)
    parser.add_argument("--opt", type=int, choices=range(4), default=3)
    parser.add_argument("--json", type=Path, help="write samples and environment metadata")
    args = parser.parse_args()
    if args.repeat < 1 or args.warmup < 1:
        parser.error("repeat and warmup must each be at least 1")
    PXR = args.pxr.resolve()
    if not PXR.exists():
        parser.error(f"{PXR} not found; run cargo build --release first")

    php = shutil.which("php")
    report = {"environment": metadata(php), "repeat": args.repeat,
              "warmup": args.warmup, "opt": args.opt,
              "timing": "process wall time, including startup; compilation excluded",
              "order": "PXR/PHP on even rounds, PHP/PXR on odd rounds",
              "results": []}
    native_env = os.environ.copy()
    native_env.pop("PXR_GC_STRESS", None)
    native_env.pop("PXR_GC_STATS", None)
    native_env["PXR_PHP"] = "/pxr-benchmark-no-php-worker"
    print(f"Optimization -O{args.opt}; {args.warmup} warmups; {args.repeat} measured runs per runtime", flush=True)
    print("Times include process startup; output equality checked on every run.", flush=True)
    if php:
        settings = report["environment"]["php_settings"]
        print(f"PHP: {report['environment']['php_version'].splitlines()[0]}", flush=True)
        print(f"OPcache CLI={settings['opcache.enable_cli']}; JIT={settings['opcache.jit']}", flush=True)
    print(f"{'benchmark':<18}{'PXR median ms':>15}{'PHP median ms':>15}{'PHP/PXR':>12}", flush=True)
    print("-" * 60, flush=True)

    with tempfile.TemporaryDirectory(prefix="pxr-bench-") as tmp:
        for bench in BENCHMARKS:
            source = BENCH_DIR / bench
            executable = Path(tmp) / source.stem
            start = time.perf_counter()
            subprocess.run([str(PXR), "build", str(source), "-o", str(executable),
                            "--opt", str(args.opt)], check=True, capture_output=True,
                           timeout=120)
            build_seconds = time.perf_counter() - start
            commands = {"pxr": [str(executable)]}
            if php:
                commands["php"] = [php, str(source)]
            samples = {name: [] for name in commands}
            expected = None
            for round_id in range(args.warmup + args.repeat):
                order = list(commands) if round_id % 2 == 0 else list(reversed(commands))
                for name in order:
                    elapsed, output = run(commands[name], native_env if name == "pxr" else None)
                    if expected is None:
                        expected = output
                    if output != expected:
                        raise RuntimeError(f"{bench}: output mismatch for {name}: "
                                           f"expected {expected!r}, got {output!r}")
                    if round_id >= args.warmup:
                        samples[name].append(elapsed)
            row = {"benchmark": bench, "source_sha256": hashlib.sha256(source.read_bytes()).hexdigest(),
                   "output": expected.decode(), "build_seconds": build_seconds,
                   **{name: summarize(times) for name, times in samples.items()}}
            report["results"].append(row)
            if php:
                ratio = row["php"]["median_seconds"] / row["pxr"]["median_seconds"]
                row["php_over_pxr_median"] = ratio
                print(f"{bench:<18}{row['pxr']['median_seconds'] * 1000:>15.3f}"
                      f"{row['php']['median_seconds'] * 1000:>15.3f}{ratio:>11.2f}x", flush=True)
            else:
                print(f"{bench:<18}{row['pxr']['median_seconds'] * 1000:>15.3f}{'n/a':>15}{'n/a':>12}", flush=True)
    print("PHP/PXR > 1: PXR faster; < 1: PHP faster. These are microbenchmarks.")
    if args.json:
        args.json.parent.mkdir(parents=True, exist_ok=True)
        args.json.write_text(json.dumps(report, indent=2) + "\n")
        print(f"Samples and metadata: {args.json}")
    return 0


if __name__ == "__main__":
    try:
        sys.exit(main())
    except (RuntimeError, subprocess.SubprocessError) as error:
        print(f"benchmark failed: {error}", file=sys.stderr)
        if isinstance(error, subprocess.CalledProcessError) and error.stderr:
            print(error.stderr.decode(errors="replace"), file=sys.stderr)
        sys.exit(1)
