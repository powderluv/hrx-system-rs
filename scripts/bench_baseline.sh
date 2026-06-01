#!/usr/bin/env bash
# Baseline performance: HRX libamdhip64 vs vanilla CLR libamdhip64, on the same
# hip_bench.c binary's host-API + memory-transfer paths. (Kernel dispatch is NOT
# benchmarked — HRX's fatbin-registered kernel path does not currently execute
# correctly on gfx942, so it would fail the correctness gate.)
#
# Run on the MI300 host. Captures results/bench_baseline.csv for regression use
# when the Rust libhrx/HIP port lands.
set -u
V="${THEROCK_VENV:-$HOME/github/therock-venv}"
DEVEL="$V/lib/python3.12/site-packages/_rocm_sdk_devel"
CORE="$V/lib/python3.12/site-packages/_rocm_sdk_core"
SYSDEPS="$DEVEL/lib/rocm_sysdeps/lib"
BUILD="${HRX_BUILD_DIR:-$HOME/github/hrx-system-build}"
HRXLIB="$BUILD/libhrx/src/libhrx"
HIPDIR="$BUILD/libhrx/src/binding/hip"
RS="$(cd "$(dirname "$0")/.." && pwd)"
OUT="$RS/results"; mkdir -p "$OUT"
BENCH="$RS/tests/apps/hip_bench.c"

CLR_LIB="$DEVEL/lib/libamdhip64.so.7"
HRX_LIB="$HIPDIR/libamdhip64.so.7.9999.0"

# Build one binary per backend (link directly so there's no preload version
# mismatch; the source is identical).
mkdir -p "$OUT/bin"
gcc "$BENCH" "$CLR_LIB" -Wl,-rpath,"$DEVEL/lib" -lrt -o "$OUT/bin/bench_clr"
gcc "$BENCH" "$HRX_LIB" -Wl,-rpath,"$HIPDIR" -Wl,-rpath,"$HRXLIB" -lrt -o "$OUT/bin/bench_hrx"

run() { # $1=label $2=binary $3..=LD_LIBRARY_PATH extra/env
  local label="$1" bin="$2"; shift 2
  echo "== $label ==" >&2
  env "$@" "$bin" 2>"$OUT/${label}.stderr"
}

echo "backend,category,name,bytes,median_ns,p10_ns,p90_ns,iters" > "$OUT/bench_baseline.csv"
collect() { # $1=label   reads RESULT lines from stdin
  awk -v b="$1" '/^RESULT/{print b","$2","$3","$4","$5","$6","$7","$8}'
}

# CLR
run clr "$OUT/bin/bench_clr" \
  LD_LIBRARY_PATH="$DEVEL/lib:$SYSDEPS" \
  | tee "$OUT/clr.out" | collect clr >> "$OUT/bench_baseline.csv"

# HRX
run hrx "$OUT/bin/bench_hrx" \
  LD_LIBRARY_PATH="$HIPDIR:$HRXLIB:$CORE/lib:$DEVEL/lib:$SYSDEPS" \
  HRX_GPU_DRIVER=amdgpu \
  | tee "$OUT/hrx.out" | collect hrx >> "$OUT/bench_baseline.csv"

echo "=== gate status ==="
echo "clr: $(grep -m1 -E 'GATE (OK|FAILED)' "$OUT/clr.out" || echo '?')"
echo "hrx: $(grep -m1 -E 'GATE (OK|FAILED)' "$OUT/hrx.out" || echo '?')"
echo "CSV: $OUT/bench_baseline.csv ($(($(wc -l < "$OUT/bench_baseline.csv")-1)) rows)"

# Pretty comparison if python is around.
python3 "$RS/scripts/bench_compare.py" "$OUT/bench_baseline.csv" 2>/dev/null || true
