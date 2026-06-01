# hrx-system-rs

A Rust port of [ROCm/hrx-system](https://github.com/ROCm/hrx-system), starting
with the **LD_PRELOAD HIP passthrough/interception** layer.

See [`plans/rust-port-plan.md`](plans/rust-port-plan.md) for scope, phasing, and
the analysis of upstream.

## Status

- **Phase 0 (baseline) — working.** The upstream C passthrough libraries build
  standalone (gcc, no ROCm/IREE), and a HIP app runs through
  `libhip_intercept.so` under `LD_PRELOAD` against a real backend
  `libamdhip64.so`, routed through the logging interceptor. This is the
  conformance oracle the Rust port is diffed against.
- **Phase 1 (Rust passthrough) — in progress.**

### GPU status (2026-05-31)

The differential test is verified **GPU-free** (host shows 0 devices): both the C
and Rust interceptors produce the identical 6-line `count=0` trace, which proves
the ABI/format port. It has **not** yet been validated against a live GPU.

Attempting to bring up the gfx1201 GPU live (it is held by vfio-pci at boot via
`vfio-pci-temp.conf` + amdgpu blacklist) wedged the card: live `modprobe -r
amdgpu` fails ("Module is in use") and sysfs PCI reset is unsupported on this
device ("Inappropriate ioctl"), so it needs a **reboot** to recover. Two earlier
commits (8d950eb, 91e99c4) overstated "GPU active" — that was a misread of
unreliable tool output; the committed golden trace is the `count=0` trace.

GPU tests must run with the Bash sandbox disabled (it masks `/dev/kfd`).

## Layout

```
plans/rust-port-plan.md     full plan + upstream analysis
scripts/build_c_baseline.sh build the upstream C passthrough libs (reference)
scripts/run_preload_test.sh run a HIP app through a passthrough .so + interceptor
tests/apps/hip_smoke.c      deterministic HIP smoke app
tests/golden/               reference trace + exported-symbol list
crates/                     Rust workspace (Phase 1+)
```

## Reproduce the Phase 0 baseline

Prereqs: a real `libamdhip64.so`. We use a TheRock nightly in a venv:

```bash
python3 -m venv ~/github/therock-nightly-venv
~/github/therock-nightly-venv/bin/pip install \
  --index-url https://rocm.nightlies.amd.com/v2/gfx120X-all/ "rocm[libraries,devel]"
```

Build the reference C passthrough libraries and run the LD_PRELOAD test:

```bash
HRX_SRC=~/github/hrx-system/libhrx/src bash scripts/build_c_baseline.sh
bash scripts/run_preload_test.sh \
  build/c-baseline/libhip_intercept.so build/c-baseline/libhip_logging.so
```

The harness auto-detects the backend `libamdhip64.so` from the venv (override
with `HIP_PASSTHROUGH_BACKEND_LIB`) and prints a normalized trace.

### Notes
- `hip_intercept.c` reads `HIP_INTERCEPTION_LIBRARY` for the interceptor path
  (the upstream README's `HIP_PASSTHROUGH_INTERCEPTOR` name applies to the
  separate `passthrough.c` target).
- The captured reference trace reflects a host with **0 visible GPUs**; device
  memory/kernel calls are exercised once a GPU is visible to the process. The
  C-vs-Rust differential test is environment-independent (both run side by side).

## GPU validation: MI300X (gfx942)

The local gfx1201 Radeon can't run the HRX amdgpu HAL (it exposes only a
COARSE GRAINED VRAM pool; IREE needs fine-grained device-local memory). An
MI300X exposes fine-grained GPU pools, so validation runs there.

`scripts/mi300_validate.sh` (run on an MI300 host with a TheRock `gfx94X-dcgpu`
nightly venv) builds the C reference, proves the HRX GPU/HIP product path works,
builds the Rust workspace, link-tests `iree-sys` against the freshly-built IREE
archives, and runs the Phase-1 differential matrix + buffer tracer on the real
GPU.

Verified on MI300X (8x gfx942, ROCm 7.14 nightly):
- HRX core ABI GPU path: `hrx_gpu_initialize`=0, device count 8, "AMD Instinct MI300X"
- HRX HIP product path (HRX's own libamdhip64.so): smoke app runs, count=8
- `iree-sys` static-links the 108 IREE archives and runs (allocator roundtrip)
- passthrough differential matrix: all 4 {C,Rust}×{C,Rust} combos identical,
  full device path (count=8, malloc/memcpy/memset/free)
- buffer tracer: plain/hash/hex identical, FNV-1a hashes match
