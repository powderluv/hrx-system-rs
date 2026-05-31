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
