#!/usr/bin/env bash
# GPU validation on an MI300X (gfx942) host, which — unlike the local gfx1201
# Radeon — exposes FINE GRAINED device-local HSA pools, so the HRX amdgpu HAL
# (and thus the HRX HIP product path) actually runs.
#
# Run ON the MI300 host after:
#   1. python3 -m venv ~/github/therock-venv
#   2. ~/github/therock-venv/bin/pip install \
#        --index-url https://rocm.nightlies.amd.com/v2/gfx94X-dcgpu/ "rocm[libraries,devel]"
#   3. pip install --user --break-system-packages ninja   (if ninja absent)
#   4. clone ROCm/hrx-system and this repo under ~/github
#
# It builds the C reference (libhrx.so + libamdhip64.so), proves the HRX GPU
# path works on gfx942, builds the Rust workspace, link-tests iree-sys against
# the freshly-built IREE archives, and runs the passthrough/interceptor
# differential matrix + buffer tracer on the real GPU.
set -u
V="${THEROCK_VENV:-$HOME/github/therock-venv}"
DEVEL="$V/lib/python3.12/site-packages/_rocm_sdk_devel"
CORE="$V/lib/python3.12/site-packages/_rocm_sdk_core"
CLANG="$DEVEL/lib/llvm/bin/clang"
CLANGXX="$DEVEL/lib/llvm/bin/clang++"
SYSDEPS="$DEVEL/lib/rocm_sysdeps/lib"
SRC="${HRX_SYSTEM_SRC:-$HOME/github/hrx-system}"
BUILD="${HRX_BUILD_DIR:-$HOME/github/hrx-system-build}"
RS="$(cd "$(dirname "$0")/.." && pwd)"
HRXLIB="$BUILD/libhrx/src/libhrx"
HIPDIR="$BUILD/libhrx/src/binding/hip"
export PATH="$HOME/.local/bin:$HOME/.cargo/bin:$PATH"
export HRX_BUILD_DIR="$BUILD" THEROCK_VENV="$V"
LP="$HRXLIB:$CORE/lib:$DEVEL/lib:$SYSDEPS"
step() { echo "=====PHASE: $1====="; }

step cmake-build
[ -f "$BUILD/build.ninja" ] || cmake -B "$BUILD" -S "$SRC" -GNinja \
  -DCMAKE_BUILD_TYPE=Release -DCMAKE_C_COMPILER="$CLANG" \
  -DCMAKE_CXX_COMPILER="$CLANGXX" -DCMAKE_PREFIX_PATH="$DEVEL" \
  -DIREE_BUILD_TESTS=OFF -DLIBHRX_BUILD_CTS=OFF -DHRX_INSTALL_TESTS=OFF \
  -DIREE_ENABLE_WERROR_FLAG=OFF
ninja -C "$BUILD"

step gpu-hip-product-path
gcc "$RS/tests/apps/hip_smoke.c" "$HIPDIR/libamdhip64.so.7" \
  -Wl,-rpath,"$HIPDIR" -Wl,-rpath,"$HRXLIB" -o /tmp/smoke_hrx
LD_LIBRARY_PATH="$HIPDIR:$LP" HRX_GPU_DRIVER=amdgpu /tmp/smoke_hrx

step rust-build-and-link
cd "$RS"
cargo build --release
cargo test -p iree-sys --release

step passthrough-differential-gpu
HRX_SRC="$SRC/libhrx/src" bash "$RS/scripts/build_c_baseline.sh" >/dev/null
bash "$RS/scripts/differential_test.sh"

step buffer-tracer-gpu
HRX_GPU_ARCH=gfx942 bash "$RS/scripts/buffer_tracer_test.sh"

step DONE
