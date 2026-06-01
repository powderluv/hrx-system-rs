#!/usr/bin/env bash
# Verify the direct-passthrough exports (the ~304 functions that bypass the
# interceptor table and dlsym->forward to the backend) behave identically under
# the C and Rust passthrough .so. Compares the address-free output of a small
# app that calls several direct functions (hipDeviceTotalMem, GetPCIBusId,
# ComputeCapability, ...) plus one table-routed string getter.
#
# Run with the Bash sandbox DISABLED for a live-GPU check (else count=0).
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"
VENV="${THEROCK_VENV:-$HOME/github/therock-nightly-venv}"

CORE="$(dirname "$(find "$VENV" -name 'libamdhip64.so*' -type f | sort | tail -1)")"
SYSDEPS="$(find "$VENV" -type d -name lib -path '*rocm_sysdeps*' | head -1)"
BK="$(find "$VENV" -name 'libamdhip64.so*' -type f | sort | tail -1)"

mkdir -p build/direct
gcc tests/apps/hip_direct.c "$BK" -Wl,-rpath,"$CORE" -o build/direct/hip_direct

run() {
  LD_LIBRARY_PATH="$CORE:$SYSDEPS" HIP_PASSTHROUGH_BACKEND_LIB="$BK" \
    LD_PRELOAD="$1" build/direct/hip_direct 2>/dev/null
}

run build/c-baseline/libhip_intercept.so   > build/direct/out_c.txt
run target/release/libhip_intercept.so     > build/direct/out_rust.txt

echo "--- C passthrough ---";    cat build/direct/out_c.txt
if diff -u build/direct/out_c.txt build/direct/out_rust.txt; then
  echo "PASS: direct-passthrough output identical (C vs Rust)"
else
  echo "FAIL: direct-passthrough output differs" >&2; exit 1
fi
