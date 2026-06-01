#!/usr/bin/env bash
# Differential test across the full passthrough x interceptor matrix.
#
# Runs the HIP smoke app under every combination of {C, Rust} passthrough .so
# and {C, Rust} interceptor .so, and asserts all normalized traces are
# byte-identical. This proves the Rust passthrough and Rust interceptor each
# match the C reference ABI + output, and that they interoperate cross-language
# (Rust passthrough loading a C interceptor and vice-versa).
#
# Run with the Bash sandbox DISABLED if validating against a live GPU (the
# sandbox masks /dev/kfd, yielding a 0-device trace). It still passes GPU-free.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

echo "== building C baseline =="
HRX_SRC="${HRX_SRC:-$HOME/github/hrx-system/libhrx/src}" bash scripts/build_c_baseline.sh >/dev/null

echo "== building Rust crates =="
cargo build --release >/dev/null

declare -A PT=( [C]=build/c-baseline/libhip_intercept.so [Rust]=target/release/libhip_intercept.so )
declare -A INT=( [C]=build/c-baseline/libhip_logging.so   [Rust]=target/release/libhip_logging.so )

REF=""
fail=0
for pt in C Rust; do
  for it in C Rust; do
    out="build/diff-${pt}pt-${it}int"
    OUT="$out" bash scripts/run_preload_test.sh "${PT[$pt]}" "${INT[$it]}" >/dev/null
    norm="$out/trace.norm.log"
    if [[ -z "$REF" ]]; then
      REF="$norm"
      echo "ref: ${pt} passthrough + ${it} interceptor ($(wc -l < "$norm") lines)"
    elif cmp -s "$REF" "$norm"; then
      echo "OK : ${pt} passthrough + ${it} interceptor — identical"
    else
      echo "DIFF: ${pt} passthrough + ${it} interceptor"; diff -u "$REF" "$norm" || true; fail=1
    fi
  done
done

if [[ $fail -eq 0 ]]; then
  echo "PASS: all 4 passthrough x interceptor combinations produce identical traces"
else
  echo "FAIL: some combinations differ" >&2; exit 1
fi
