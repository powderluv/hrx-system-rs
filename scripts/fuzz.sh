#!/usr/bin/env bash
# Run the cargo-fuzz lifecycle target(s) under libfuzzer + ASAN (plan C3). The
# stateful value_list lifecycle fuzzer is GPU-free, but the fuzz binary links the
# IREE static archives, so it runs where the prebuilt hrx-system build exists
# ($HRX_BUILD_DIR) — locally on a dev box with the build, or the MI300 runner.
#
# Usage: fuzz.sh [seconds] [target]
#   seconds: per-target wall-clock budget (default 60; CI uses a short smoke)
#   target:  run only this one (default: every target below, sequentially)
set -euo pipefail
SECS="${1:-60}"
TOOLCHAIN="${FUZZ_TOOLCHAIN:-nightly-2026-04-03}"
cd "$(dirname "$0")/.."

command -v cargo-fuzz >/dev/null 2>&1 || cargo "+$TOOLCHAIN" install cargo-fuzz

if [ -n "${2:-}" ]; then
  TARGETS=("$2")
else
  TARGETS=(value_list_lifecycle buffer_lifecycle)
fi

for t in "${TARGETS[@]}"; do
  echo "== fuzzing $t for ${SECS}s =="
  cargo "+$TOOLCHAIN" fuzz run "$t" -- -max_total_time="$SECS" -rss_limit_mb=4096
done
