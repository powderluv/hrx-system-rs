#!/usr/bin/env bash
# Differential test for the Rust libhrx port: run hrx_abi_test.c against the C
# libhrx.so and the Rust libhrx_rs.so, assert identical output. GPU-independent
# (status/host_allocator/value_list) — runs anywhere, but we validate on MI300.
set -u
RS="$(cd "$(dirname "$0")/.." && pwd)"
V="${THEROCK_VENV:-$HOME/github/therock-venv}"
DEVEL="$V/lib/python3.12/site-packages/_rocm_sdk_devel"
CORE="$V/lib/python3.12/site-packages/_rocm_sdk_core"
SYSDEPS="$DEVEL/lib/rocm_sysdeps/lib"
BUILD="${HRX_BUILD_DIR:-$HOME/github/hrx-system-build}"
C_HRXLIB="$BUILD/libhrx/src/libhrx"
export PATH="$HOME/.cargo/bin:$PATH"
export HRX_BUILD_DIR="$BUILD"
OUT="$RS/build/libhrx-diff"; mkdir -p "$OUT"

echo "== build Rust libhrx_rs.so =="
( cd "$RS" && cargo build --release -p hrx-libhrx ) || exit 1
RUST_LIB="$RS/target/release/libhrx_rs.so"
C_LIB="$C_HRXLIB/libhrx.so.0"

APP="$RS/tests/apps/hrx_abi_test.c"
# Build one binary per backend (link directly; both export the same hrx_* ABI).
gcc "$APP" "$C_LIB"    -Wl,-rpath,"$C_HRXLIB" -o "$OUT/abi_c"    || exit 1
gcc "$APP" "$RUST_LIB" -Wl,-rpath,"$RS/target/release" -o "$OUT/abi_rust" || exit 1

# The C libhrx.so pulls in IREE which may want HSA at load even for these calls;
# provide the ROCm libs on the path. The Rust .so links IREE statically.
LP="$C_HRXLIB:$CORE/lib:$DEVEL/lib:$SYSDEPS"
echo "== run C backend =="
LD_LIBRARY_PATH="$LP" "$OUT/abi_c"   > "$OUT/c.out" 2>&1; echo "C rc=$?"
echo "== run Rust backend =="
LD_LIBRARY_PATH="$LP" "$OUT/abi_rust" > "$OUT/rust.out" 2>&1; echo "Rust rc=$?"

echo "== C output =="; cat "$OUT/c.out"
echo "== diff (C vs Rust) =="
if diff -u "$OUT/c.out" "$OUT/rust.out"; then
  echo "PASS: Rust libhrx output identical to C libhrx"
else
  echo "FAIL: outputs differ"; exit 1
fi
