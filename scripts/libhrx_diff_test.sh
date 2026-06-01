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

# The C libhrx.so pulls in IREE which may want HSA at load; provide the ROCm
# libs on the path. The Rust .so links IREE statically.
LP="$C_HRXLIB:$CORE/lib:$DEVEL/lib:$SYSDEPS"

fail=0
run_diff() { # $1=test-name $2=source.c
  local name="$1" app="$RS/tests/apps/$2"
  gcc "$app" "$C_LIB"    -Wl,-rpath,"$C_HRXLIB" -o "$OUT/${name}_c"    || return 1
  gcc "$app" "$RUST_LIB" -Wl,-rpath,"$RS/target/release" -o "$OUT/${name}_rust" || return 1
  LD_LIBRARY_PATH="$LP" "$OUT/${name}_c"    > "$OUT/${name}_c.out"    2>&1; local rc_c=$?
  LD_LIBRARY_PATH="$LP" "$OUT/${name}_rust" > "$OUT/${name}_rust.out" 2>&1; local rc_r=$?
  echo "== [$name] C output (rc=$rc_c) =="; cat "$OUT/${name}_c.out"
  if diff -u "$OUT/${name}_c.out" "$OUT/${name}_rust.out" && [ "$rc_c" = "$rc_r" ]; then
    echo "OK : [$name] Rust identical to C (rc both=$rc_c)"
  else
    echo "DIFF: [$name] (rc_c=$rc_c rc_rust=$rc_r)"; fail=1
  fi
}

run_diff abi  hrx_abi_test.c    # status + host_allocator (init-free)
run_diff init hrx_init_test.c   # cpu init + device + value_list

if [ "$fail" = 0 ]; then
  echo "PASS: Rust libhrx output identical to C libhrx (all suites)"
else
  echo "FAIL: outputs differ"; exit 1
fi
