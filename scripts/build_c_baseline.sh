#!/usr/bin/env bash
# Build the upstream C passthrough LD_PRELOAD libraries standalone (no ROCm/IREE).
# These are the reference ("golden") binaries the Rust port is diffed against.
set -euo pipefail

HRX_SRC="${HRX_SRC:-$HOME/github/hrx-system/libhrx/src}"
OUT="${OUT:-$(cd "$(dirname "$0")/.." && pwd)/build/c-baseline}"
PT="$HRX_SRC/passthrough"

if [[ ! -d "$PT" ]]; then
  echo "error: passthrough sources not found at $PT (set HRX_SRC)" >&2
  exit 1
fi

mkdir -p "$OUT"
CFLAGS=(-shared -fPIC -D_GNU_SOURCE -O2 -Wall -I"$HRX_SRC")

echo "Building passthrough libraries into $OUT"
gcc "${CFLAGS[@]}" "$PT/hip_intercept.c"                       -ldl -lpthread -o "$OUT/libhip_intercept.so"
gcc "${CFLAGS[@]}" "$PT/hip_logging.c"                                        -o "$OUT/libhip_logging.so"
gcc "${CFLAGS[@]}" "$PT/hip_buffer_tracer.c"                   -ldl           -o "$OUT/libhip_buffer_tracer.so"
gcc "${CFLAGS[@]}" "$PT/interceptors/passthrough_interceptor.c"               -o "$OUT/libhip_noop.so"

echo "Built:"
ls -l "$OUT"/*.so
echo "Exported HIP symbols in libhip_intercept.so:"
nm -D --defined-only "$OUT/libhip_intercept.so" | grep -c ' T hip' || true
