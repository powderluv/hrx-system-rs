#!/usr/bin/env bash
# Run the cargo-fuzz lifecycle target(s) under libfuzzer + ASAN (plan C3). The
# stateful value_list lifecycle fuzzer is GPU-free, but the fuzz binary links the
# IREE static archives, so it runs where the prebuilt hrx-system build exists
# ($HRX_BUILD_DIR) — locally on a dev box with the build, or the MI300 runner.
#
# Usage: fuzz.sh [seconds]   (default 60; CI uses a short smoke)
set -euo pipefail
SECS="${1:-60}"
TOOLCHAIN="${FUZZ_TOOLCHAIN:-nightly-2026-04-03}"
cd "$(dirname "$0")/.."

command -v cargo-fuzz >/dev/null 2>&1 || cargo "+$TOOLCHAIN" install cargo-fuzz

cargo "+$TOOLCHAIN" fuzz run value_list_lifecycle -- \
  -max_total_time="$SECS" -rss_limit_mb=4096
