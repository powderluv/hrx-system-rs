#!/usr/bin/env bash
# Run the libhrx Conformance Test Suite (from hrx-system, built with
# -DLIBHRX_BUILD_CTS=ON) against a libhrx implementation. The CTS dlopens the
# `--hrx-library`, so one CTS build tests either the C libhrx or the Rust port.
# Defaults to the CPU device (HRX_CTS_DEVICE=cpu) so the suite is GPU-free. Run ON
# the MI300 host (or any host with the CTS built + the prebuilt hrx-system build).
#
# Usage: cts.sh [path-to-libhrx.so]   (default: the freshly-built Rust libhrx_rs.so)
set -u
RS="$(cd "$(dirname "$0")/.." && pwd)"
V="${THEROCK_VENV:-$HOME/github/therock-venv}"
DEVEL="$V/lib/python3.12/site-packages/_rocm_sdk_devel"
CORE="$V/lib/python3.12/site-packages/_rocm_sdk_core"
SYSDEPS="$DEVEL/lib/rocm_sysdeps/lib"
BUILD="${HRX_BUILD_DIR:-$HOME/github/hrx-system-build}"
CTS="$BUILD/libhrx/cts"
C_HRXLIB="$BUILD/libhrx/src/libhrx"
export PATH="$HOME/.cargo/bin:$PATH"
export HRX_BUILD_DIR="$BUILD"
export HRX_CTS_DEVICE="${HRX_CTS_DEVICE:-cpu}"
LP="$C_HRXLIB:$CORE/lib:$DEVEL/lib:$SYSDEPS"

echo "== build Rust libhrx_rs.so =="
( cd "$RS" && cargo build --release -p hrx-libhrx ) || exit 1
RUST_LIB="$RS/target/release/libhrx_rs.so"

LIB="${1:-$RUST_LIB}"
# Categories that work on the local-task CPU device (no kernel binary required).
# executable / queue_ops need a compiled kernel and are excluded here (see the CTS
# README "Not Yet Tested").
CATS="status lifecycle device host_allocator allocator semaphore stream memory transfer stream_ops refcount virtual_memory"

# Guard: if the CTS was not built, fail loudly rather than passing vacuously.
have=0
for c in $CATS; do [ -x "$CTS/hrx_cts_$c" ] && have=$((have + 1)); done
if [ "$have" = 0 ]; then
  echo "FAIL: no CTS binaries in $CTS — reconfigure the hrx-system build with"
  echo "      -DLIBHRX_BUILD_CTS=ON -DBUILD_TESTING=ON and rebuild."
  exit 1
fi

echo "== CTS against $LIB (device=$HRX_CTS_DEVICE) =="
fail=0
for c in $CATS; do
  bin="$CTS/hrx_cts_$c"
  [ -x "$bin" ] || { echo "SKIP : $c (no binary — build with -DLIBHRX_BUILD_CTS=ON)"; continue; }
  LD_LIBRARY_PATH="$LP" "$bin" --hrx-library "$LIB" >"/tmp/cts_$c.out" 2>&1
  rc=$?
  if [ "$rc" -eq 0 ]; then
    echo "PASS : $c $(grep -oE '[0-9]+ assertion(s)? in [0-9]+ test case(s)?' "/tmp/cts_$c.out" | tail -1)"
  else
    echo "FAIL : $c (rc=$rc)"; tail -8 "/tmp/cts_$c.out"; fail=1
  fi
done

if [ "$fail" = 0 ]; then
  echo "PASS: all CTS categories conform"
else
  echo "FAIL: some CTS categories"; exit 1
fi
