#!/usr/bin/env bash
# Run the HIP smoke app through a passthrough libamdhip64 replacement under
# LD_PRELOAD, with the logging interceptor, and emit a normalized trace.
#
# Usage:
#   run_preload_test.sh <intercept.so> [interceptor.so]
#
# Env:
#   HIP_PASSTHROUGH_BACKEND_LIB  real libamdhip64.so (auto-detected from the
#                                TheRock nightly venv if unset)
#   OUT                          output dir (default build/test-out)
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
INTERCEPT="${1:?usage: run_preload_test.sh <intercept.so> [interceptor.so]}"
INTERCEPTOR="${2:-$ROOT/build/c-baseline/libhip_logging.so}"
OUT="${OUT:-$ROOT/build/test-out}"
VENV="${THEROCK_VENV:-$HOME/github/therock-nightly-venv}"
mkdir -p "$OUT"

# Locate the real backend libamdhip64.so from the nightly venv if not provided.
if [[ -z "${HIP_PASSTHROUGH_BACKEND_LIB:-}" ]]; then
  HIP_PASSTHROUGH_BACKEND_LIB="$(find "$VENV" -name 'libamdhip64.so*' -type f 2>/dev/null | sort | tail -1 || true)"
fi
if [[ -z "${HIP_PASSTHROUGH_BACKEND_LIB:-}" || ! -e "$HIP_PASSTHROUGH_BACKEND_LIB" ]]; then
  echo "error: real libamdhip64.so not found; set HIP_PASSTHROUGH_BACKEND_LIB" >&2
  exit 1
fi
export HIP_PASSTHROUGH_BACKEND_LIB
echo "backend     : $HIP_PASSTHROUGH_BACKEND_LIB"
echo "passthrough : $INTERCEPT"
echo "interceptor : $INTERCEPTOR"

# The backend's own deps (libamd_comgr, hsa, etc.) live alongside it.
BACKEND_DIR="$(dirname "$HIP_PASSTHROUGH_BACKEND_LIB")"
export LD_LIBRARY_PATH="$BACKEND_DIR:${LD_LIBRARY_PATH:-}"

# Build the smoke app, linking against the real backend by full path (as a real
# HIP app would link -lamdhip64). At runtime LD_PRELOAD interposes the
# passthrough's hip* symbols ahead of the backend's.
APP="$OUT/hip_smoke"
if [[ ! -x "$APP" ]]; then
  gcc "$ROOT/tests/apps/hip_smoke.c" "$HIP_PASSTHROUGH_BACKEND_LIB" \
    -Wl,-rpath,"$BACKEND_DIR" -o "$APP"
fi

RAW="$OUT/trace.raw.log"
NORM="$OUT/trace.norm.log"
# NOTE: hip_intercept.c reads HIP_INTERCEPTION_LIBRARY (the README's
# HIP_PASSTHROUGH_INTERCEPTOR name applies to passthrough.c, a different target).
HIP_LOG_FILE="$RAW" HIP_LOG_LEVEL=2 \
  HIP_INTERCEPTION_LIBRARY="$INTERCEPTOR" \
  LD_PRELOAD="$INTERCEPT" "$APP" 2> "$OUT/app.stderr" || {
    echo "app exited non-zero; see $OUT/app.stderr" >&2; cat "$OUT/app.stderr" >&2; }

# Normalize volatile fields so traces are comparable across runs / impls:
#   [123.456789]  -> [TS]
#   0x7f…         -> 0xPTR
#   version=NNNN  -> version=VER
sed -E -e 's/^\[[0-9]+\.[0-9]+\]/[TS]/' \
       -e 's/0x[0-9a-fA-F]+/0xPTR/g' \
       -e 's/(version=)[0-9]+/\1VER/g' \
       "$RAW" > "$NORM"

echo "--- normalized trace ($NORM) ---"
cat "$NORM"
