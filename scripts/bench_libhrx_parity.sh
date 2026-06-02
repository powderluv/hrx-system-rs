#!/usr/bin/env bash
# Public-ABI parity microbench: time the hrx_* public ABI through the C
# libhrx.so.0 AND the Rust libhrx_rs.so (same hrx_bench.c source), on a real GPU
# (gfx942/MI300X). Produces results/bench_libhrx_parity.csv with backends
# {c-hrx, rust} and renders ratio = rust / c-hrx (~1.0 == parity).
#
# Run ON the MI300 host:
#   THEROCK_VENV=~/github/therock-venv HRX_BUILD_DIR=~/github/hrx-system-build \
#     bash scripts/bench_libhrx_parity.sh
set -u
RS="$(cd "$(dirname "$0")/.." && pwd)"
V="${THEROCK_VENV:-$HOME/github/therock-venv}"
DEVEL="$V/lib/python3.12/site-packages/_rocm_sdk_devel"
CORE="$V/lib/python3.12/site-packages/_rocm_sdk_core"
SYSDEPS="$DEVEL/lib/rocm_sysdeps/lib"
BUILD="${HRX_BUILD_DIR:-$HOME/github/hrx-system-build}"
C_HRXLIB="$BUILD/libhrx/src/libhrx"
C_LIB="$C_HRXLIB/libhrx.so.0"
export PATH="$HOME/.cargo/bin:$PATH"
export HRX_BUILD_DIR="$BUILD"

BENCH="$RS/tests/apps/hrx_bench.c"
OUT="$RS/results"
BIN="$OUT/bin"
CSV="$OUT/bench_libhrx_parity.csv"
mkdir -p "$BIN"

echo "== build Rust libhrx_rs.so =="
( cd "$RS" && cargo build --release -p hrx-libhrx ) || exit 1
RUST_LIB="$RS/target/release/libhrx_rs.so"

echo "== build bench against each backend =="
gcc "$BENCH" "$C_LIB"    -Wl,-rpath,"$C_HRXLIB"          -lrt -o "$BIN/hrx_bench_c"    || exit 1
gcc "$BENCH" "$RUST_LIB" -Wl,-rpath,"$RS/target/release" -lrt -o "$BIN/hrx_bench_rust" || exit 1

# C libhrx dynamically needs ROCm/HSA at load; Rust links IREE statically but the
# amdgpu HAL still dlopen's HSA, so both get the venv libs on the path.
LP="$C_HRXLIB:$CORE/lib:$DEVEL/lib:$SYSDEPS"

collect() { awk -v b="$1" '/^RESULT/{print b","$2","$3","$4","$5","$6","$7","$8}'; }

echo "backend,category,name,bytes,median_ns,p10_ns,p90_ns,iters" > "$CSV"

# Run one backend binary to a captured .out, validate it actually produced
# trustworthy data (clean exit + GATE OK + at least one RESULT row) BEFORE
# appending to the CSV. A crashed / gate-failed backend must NOT silently leave
# the column empty-but-"successful" — the parity table would then read as
# bogus parity. Returns nonzero (recorded in $RC) on any failure.
run_backend() { # $1=label  $2=binary
  local label="$1" bin="$2"
  LD_LIBRARY_PATH="$LP" HRX_GPU_DRIVER=amdgpu "$bin" \
    > "$OUT/$label.out" 2> "$OUT/$label.stderr"
  local rc=$?
  if [ "$rc" -ne 0 ]; then
    echo "FAIL: $label exited rc=$rc"; RC=1; return 1
  fi
  if ! grep -q '^GATE OK$' "$OUT/$label.out"; then
    echo "FAIL: $label did not pass the correctness gate"; RC=1; return 1
  fi
  if ! grep -q '^RESULT ' "$OUT/$label.out"; then
    echo "FAIL: $label produced no RESULT rows"; RC=1; return 1
  fi
  collect "$label" < "$OUT/$label.out" >> "$CSV"
}

RC=0
echo "== run c-hrx =="; run_backend c-hrx "$BIN/hrx_bench_c"
echo "== run rust =="; run_backend rust "$BIN/hrx_bench_rust"

echo "== gates =="
echo "c-hrx: $(grep -m1 -E 'GATE (OK|FAILED)' "$OUT/c-hrx.out" || echo '?') $(grep -m1 fill_supported "$OUT/c-hrx.stderr" || true)"
echo "rust:  $(grep -m1 -E 'GATE (OK|FAILED)' "$OUT/rust.out" || echo '?') $(grep -m1 fill_supported "$OUT/rust.stderr" || true)"
grep -h '^SKIP ' "$OUT/c-hrx.stderr" "$OUT/rust.stderr" 2>/dev/null && echo "(note: SKIP rows above were dropped — an op errored on that backend)"
[ "$RC" -ne 0 ] && echo "WARNING: a backend failed validation; the parity table below is incomplete."

echo "== parity table (ratio = rust / c-hrx) =="
python3 "$RS/scripts/bench_compare.py" "$CSV" c-hrx 2>/dev/null || true

echo "== perf gate =="
if [ "$RC" -ne 0 ]; then
  echo "PERF GATE FAILED: a backend did not produce trustworthy data (see above)."
  exit 1
fi
python3 "$RS/scripts/bench_gate.py" "$CSV" c-hrx rust
exit $?
