#!/usr/bin/env bash
# ABI gate (GPU-free): the Rust libhrx_rs.so must export EXACTLY the same set of
# hrx_* symbols (functions + data) as the C libhrx.so.0 — no missing, no extra.
# Catches accidental ABI surface drift in the Rust port.
#
#   HRX_BUILD_DIR=~/github/hrx-system-build bash scripts/check_symbol_parity.sh
set -u
RS="$(cd "$(dirname "$0")/.." && pwd)"
BUILD="${HRX_BUILD_DIR:-$HOME/github/hrx-system-build}"
C_LIB="$BUILD/libhrx/src/libhrx/libhrx.so.0"
export PATH="$HOME/.cargo/bin:$PATH"

[ -f "$C_LIB" ] || { echo "FAIL: C libhrx not found at $C_LIB (set HRX_BUILD_DIR)"; exit 2; }
( cd "$RS" && cargo build --release -p hrx-libhrx ) || exit 1
RUST_LIB="$RS/target/release/libhrx_rs.so"

# Defined, exported (dynamic) symbols whose name starts with hrx_ — any type
# (T/D/B/R/W), so the exported data global hrx_host_allocator_system_value counts.
exp() { nm -D --defined-only "$1" 2>/dev/null | awk '$2 ~ /^[A-Za-z]$/ && $3 ~ /^hrx_/ {print $3}' | sort -u; }

C_SET="$(exp "$C_LIB")"
R_SET="$(exp "$RUST_LIB")"
ONLY_C="$(comm -23 <(printf '%s\n' "$C_SET") <(printf '%s\n' "$R_SET"))"
ONLY_R="$(comm -13 <(printf '%s\n' "$C_SET") <(printf '%s\n' "$R_SET"))"

echo "c-libhrx hrx_* exports : $(printf '%s\n' "$C_SET" | grep -c .)"
echo "rust     hrx_* exports : $(printf '%s\n' "$R_SET" | grep -c .)"

rc=0
if [ -n "$ONLY_C" ]; then echo; echo "MISSING in rust (in C, not in Rust):"; printf '  %s\n' $ONLY_C; rc=1; fi
if [ -n "$ONLY_R" ]; then echo; echo "EXTRA in rust (in Rust, not in C):";   printf '  %s\n' $ONLY_R; rc=1; fi

if [ "$rc" = 0 ]; then echo "PASS: hrx_* symbol set identical to C libhrx"; else echo; echo "FAIL: hrx_* symbol set differs"; fi
exit $rc
