#!/usr/bin/env bash
# Differential test for the buffer tracer: run a real-kernel HIP app through the
# C passthrough with the C and the Rust buffer tracer, in three modes, and
# assert byte-identical normalized traces:
#   1. plain   (HIP_TRACE_LEVEL=3)
#   2. hash    (HIP_TRACE_SYNC=1 HIP_TRACE_DUMP=2)  — exercises FNV-1a hashing
#   3. hex     (HIP_TRACE_SYNC=1 HIP_TRACE_DUMP=1)  — exercises hex buffer dump
#
# Run with the Bash sandbox DISABLED (needs the live GPU; the kernel app uses a
# real device). Requires hipcc + a TheRock nightly venv.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"
VENV="${THEROCK_VENV:-$HOME/github/therock-nightly-venv}"
CORE="$(dirname "$(find "$VENV" -name 'libamdhip64.so*' -type f | sort | tail -1)")"
SYSDEPS="$(find "$VENV" -type d -name lib -path '*rocm_sysdeps*' | head -1)"
BK="$(find "$VENV" -name 'libamdhip64.so*' -type f | sort | tail -1)"
PT=build/c-baseline/libhip_intercept.so
ARCH="${HRX_GPU_ARCH:-gfx1201}"

HRX_SRC="${HRX_SRC:-$HOME/github/hrx-system/libhrx/src}" bash scripts/build_c_baseline.sh >/dev/null
cargo build --release >/dev/null

mkdir -p build/kernel
"$VENV/bin/hipcc" --offload-arch="$ARCH" tests/apps/hip_kernel.hip -o build/kernel/hip_kernel 2>/dev/null
APP=build/kernel/hip_kernel

run() { # $1=tracer.so $2=logfile  $3...=extra env assignments
  local tracer="$1" log="$2"; shift 2
  env LD_LIBRARY_PATH="$CORE:$SYSDEPS" HIP_PASSTHROUGH_BACKEND_LIB="$BK" \
      HIP_TRACE_LEVEL=3 HIP_INTERCEPTION_LIBRARY="$tracer" HIP_TRACE_FILE="$log" \
      "$@" LD_PRELOAD="$PT" "$APP" >/dev/null 2>&1
}
norm() { sed -E -e 's/^\[[0-9]+\.[0-9]+\]/[TS]/' -e 's/0x[0-9a-fA-F]+/0xPTR/g' "$1"; }

C=build/c-baseline/libhip_buffer_tracer.so
R=target/release/libhip_buffer_tracer.so
fail=0
check() { # name + env...
  local name="$1"; shift
  run "$C" "build/tr_c_$name.log"   "$@"
  run "$R" "build/tr_rust_$name.log" "$@"
  if diff <(norm "build/tr_c_$name.log") <(norm "build/tr_rust_$name.log") >/dev/null; then
    echo "OK : $name — identical"
  else
    echo "DIFF: $name"; diff <(norm "build/tr_c_$name.log") <(norm "build/tr_rust_$name.log") || true; fail=1
  fi
}

check plain
check hash HIP_TRACE_SYNC=1 HIP_TRACE_DUMP=2
check hex  HIP_TRACE_SYNC=1 HIP_TRACE_DUMP=1 HIP_TRACE_DUMP_MAX=64

# Independent proof the deterministic content matches (not just masked away):
hashes_c=$(grep -oE 'hash=0x[0-9a-f]+' build/tr_c_hash.log | sort)
hashes_r=$(grep -oE 'hash=0x[0-9a-f]+' build/tr_rust_hash.log | sort)
[[ "$hashes_c" == "$hashes_r" && -n "$hashes_c" ]] && echo "OK : FNV-1a hashes match ($(echo "$hashes_c" | wc -l) dumps)" || { echo "DIFF: hashes"; fail=1; }

if [[ $fail -eq 0 ]]; then
  echo "PASS: Rust buffer tracer matches C across plain/hash/hex modes"
else
  echo "FAIL: buffer tracer differs" >&2; exit 1
fi
