#!/usr/bin/env bash
# Build the PXR compiler in release mode and run the benchmark harness.
set -euo pipefail
cd "$(dirname "$0")/.."
cargo build --release -p pxr
exec python3 benchmarks/bench.py "$@"
