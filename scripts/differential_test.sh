#!/usr/bin/env bash
# Differential test: run the HIP smoke app through the C passthrough with both
# the C and the Rust logging interceptor, and assert the normalized traces are
# byte-identical. Proves the Rust interceptor matches the C reference ABI+output.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

echo "== building C baseline =="
HRX_SRC="${HRX_SRC:-$HOME/github/hrx-system/libhrx/src}" bash scripts/build_c_baseline.sh >/dev/null

echo "== building Rust crates =="
cargo build --release >/dev/null

PASSTHROUGH=build/c-baseline/libhip_intercept.so
C_INT=build/c-baseline/libhip_logging.so
RUST_INT=target/release/libhip_logging.so

echo "== running C interceptor =="
OUT=build/diff-c   bash scripts/run_preload_test.sh "$PASSTHROUGH" "$C_INT"   >/dev/null
echo "== running Rust interceptor =="
OUT=build/diff-rust bash scripts/run_preload_test.sh "$PASSTHROUGH" "$RUST_INT" >/dev/null

if diff -u build/diff-c/trace.norm.log build/diff-rust/trace.norm.log; then
  echo "PASS: Rust interceptor trace identical to C interceptor"
else
  echo "FAIL: traces differ" >&2
  exit 1
fi
