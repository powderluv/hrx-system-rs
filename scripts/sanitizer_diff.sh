#!/usr/bin/env bash
# Build the Rust libhrx with a sanitizer and run the GPU-free test apps under it,
# failing if the sanitizer reports any error (plan C2). This instruments the Rust
# side (our code + the inlined wrappers); the statically-linked IREE archives are
# not instrumented, but ASAN intercepts malloc/free process-wide, so heap errors
# (use-after-free, double-free, heap overflow) are caught across the FFI boundary
# regardless of which side allocated.
#
# Valid sanitizers (rustc `-Zsanitizer`): `address` (the GATE; heap UAF /
# double-free / overflow — validated clean on the GPU-free suites) and `thread`
# (INVESTIGATION-ONLY, not a gate). TSAN cannot be made clean here: IREE is
# statically linked and uninstrumented, so all its internal thread synchronization
# is invisible to TSAN and it reports irreducible false races inside IREE's task
# queue / worker pool (see scripts/tsan_suppressions.txt). A meaningful TSAN lane
# would require a `-fsanitize=thread` IREE build (out of scope). There is
# intentionally NO `undefined`: rustc has no UBSAN; Rust UB is covered by the Miri
# lane (scripts/miri.sh). This script rejects `undefined` with that pointer.
#
# Runtime-compatibility note: rustc uses LLVM's sanitizer runtime, so the C test
# programs MUST be compiled with clang (LLVM ASAN), never gcc (libasan) — mixing
# the two runtimes deadlocks/aborts. We use the ROCm-bundled clang.
#
# Only the GPU-free suites run here: the amdgpu/HSA path dlopens the GPU runtime,
# which does not play well with ASAN, and "GPU-free targets first" is the plan's
# guidance. Run ON the MI300 host where the IREE archives + C libhrx exist.
#
# Usage: sanitizer_diff.sh [address|thread]
set -u
SAN="${1:-address}"
if [ "$SAN" = "undefined" ]; then
  echo "rustc has no UBSAN (-Zsanitizer=undefined). Rust UB is covered by the Miri" >&2
  echo "lane: run scripts/miri.sh instead. Valid here: address | thread." >&2
  exit 2
fi
RS="$(cd "$(dirname "$0")/.." && pwd)"
V="${THEROCK_VENV:-$HOME/github/therock-venv}"
DEVEL="$V/lib/python3.12/site-packages/_rocm_sdk_devel"
CORE="$V/lib/python3.12/site-packages/_rocm_sdk_core"
SYSDEPS="$DEVEL/lib/rocm_sysdeps/lib"
CLANG="$DEVEL/lib/llvm/bin/clang"
BUILD="${HRX_BUILD_DIR:-$HOME/github/hrx-system-build}"
C_HRXLIB="$BUILD/libhrx/src/libhrx"
TOOLCHAIN="${SAN_TOOLCHAIN:-nightly-2026-04-03}"
TARGET="x86_64-unknown-linux-gnu"
export PATH="$HOME/.cargo/bin:$PATH"
export HRX_BUILD_DIR="$BUILD"
OUT="$RS/build/libhrx-san"; mkdir -p "$OUT"
LP="$C_HRXLIB:$CORE/lib:$DEVEL/lib:$SYSDEPS"

echo "== build Rust libhrx_rs.so with -Zsanitizer=$SAN =="
# -Zbuild-std rebuilds std with the sanitizer so its allocations carry redzones
# too; required for a clean ASAN link of a cdylib.
RUSTFLAGS="-Zsanitizer=$SAN" \
  cargo "+$TOOLCHAIN" build --release -p hrx-libhrx \
  -Zbuild-std --target "$TARGET" || { echo "SAN-BUILD-FAIL"; exit 1; }
RUST_LIB="$RS/target/$TARGET/release/libhrx_rs.so"
[ -f "$RUST_LIB" ] || { echo "SAN-BUILD-FAIL: no $RUST_LIB"; exit 1; }

export ASAN_OPTIONS="${ASAN_OPTIONS:-detect_leaks=0:detect_odr_violation=0:halt_on_error=1}"
# TSAN is best-effort (non-instrumented IREE → boundary false positives); apply the
# documented suppressions for the IREE-synchronized handoffs.
export TSAN_OPTIONS="${TSAN_OPTIONS:-halt_on_error=1:suppressions=$RS/scripts/tsan_suppressions.txt}"

fail=0
for t in abi:hrx_abi_test.c init:hrx_init_test.c mem:hrx_mem_test.c \
         stream:hrx_stream_test.c fence:hrx_fence_test.c queue:hrx_queue_test.c; do
  name="${t%%:*}"; src="${t##*:}"
  "$CLANG" -fsanitize="$SAN" "$RS/tests/apps/$src" "$RUST_LIB" \
    -Wl,-rpath,"$(dirname "$RUST_LIB")" -o "$OUT/${name}_san" 2>"$OUT/${name}.build" || {
      echo "SAN-LINK-FAIL [$name]"; cat "$OUT/${name}.build"; fail=1; continue; }
  LD_LIBRARY_PATH="$LP" "$OUT/${name}_san" > "$OUT/${name}.out" 2>&1; rc=$?
  if [ "$rc" -ne 0 ] || grep -qE "ERROR: (Address|Leak)Sanitizer|runtime error:|SUMMARY: (Address|Undefined)Behavior" "$OUT/${name}.out"; then
    echo "SAN-FAIL [$name] (rc=$rc):"; sed -n '1,40p' "$OUT/${name}.out"; fail=1
  else
    echo "OK : [$name] clean under $SAN (rc=$rc)"
  fi
done

if [ "$fail" = 0 ]; then
  echo "PASS: GPU-free suites clean under $SAN"
else
  echo "FAIL: $SAN reported errors"; exit 1
fi
