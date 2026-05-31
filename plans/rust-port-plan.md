# Rust Port of ROCm/hrx-system — Implementation Plan

Status: draft (2026-05-31). Owner: powderluv. Upstream: https://github.com/ROCm/hrx-system

## 1. What hrx-system actually is

The upstream repo is two very different bodies of code stitched together by a dual
Bazel + CMake build:

| Subtree | LOC (C/C++) | What it is |
|---------|-------------|------------|
| `runtime/` | ~635,000 | The **IREE runtime** subtree (upstream IREE, lightly modified). Keeps `IREE_*` options and `iree::` targets. |
| `libhrx/` | ~56,000 | The **HRX-specific** layer: public C ABI, HIP/CUDA bindings, passthrough interceptors, streaming, CTS. |

A "full Rust port" of all ~690K LOC (including embedded IREE) is not a realistic
near-term effort and would amount to rewriting IREE. **Scope is therefore the
`libhrx/` layer, with the IREE runtime consumed via FFI** (decision below).

### libhrx three-layer stack (verified)

```
        HIP app                         HRX-native app
           │                                  │
           ▼                                  ▼
 libamdhip64.so (HIP binding)         libhrx.so (public C ABI)
   240 HIP funcs, 14.2K LOC            111 hrx_* funcs, ~3.9K LOC
           │                                  │
           └──────────────┬───────────────────┘
                          ▼
            IREE streaming HAL (libhrx/src/streaming, ~11.4K LOC)  [shared]
                          ▼
            IREE core runtime (runtime/, VM + HAL + async)
```

Key facts (from source):
- `libhrx.so` (`hrx_runtime.h`, 111 `HRX_API` functions) is a thin layer over the
  IREE runtime. `hrx_internal.h` includes `iree/vm/api.h`, `iree/hal/api.h`,
  `iree/modules/hal/*`, etc. and creates an IREE VM instance at startup.
- The HIP binding `libamdhip64.so.7` (`libhrx/src/binding/hip/api.c`, 240 funcs)
  sits on the **streaming HAL directly**, not on `libhrx.so`. e.g.
  `hipStreamCreate` → `iree_hal_streaming_stream_create(...)`.
- The CMake build is **monolithic**: `libhrx/CMakeLists.txt` hard-fails unless the
  reduced IREE runtime has already configured (`if(NOT TARGET iree_vm_vm) FATAL`).
  There is no supported "libhrx-only" build that skips the IREE runtime.

## 2. The two "LD_PRELOAD paths"

The phrase "LD_PRELOAD path" is overloaded in this repo. There are two:

1. **Passthrough interception tools** — `libhrx/src/passthrough/`. A drop-in
   `libamdhip64.so` replacement (`libhip_intercept.so`) plus pluggable interceptor
   `.so`s (logging, buffer tracer, no-op). **Self-contained C** (only libc/dl/
   pthread), gcc-buildable, no ROCm or IREE needed to *build*. These are
   developer/debug tools that forward to a real HIP backend.
2. **HRX HIP-compat** — preloading the HRX-provided `libamdhip64.so` (the binding
   above) to route a HIP application through HRX/IREE. This is the *product* path
   and requires the full monolithic build + a GPU.

Decision: **do both, passthrough first** (it is the fast, GPU-optional path and a
natural first Rust target), HIP-compat in a later phase.

### Passthrough subtree inventory (`libhrx/src/passthrough/`, 14.3K LOC)

| File | LOC | Role |
|------|-----|------|
| `hip_intercept.c` | 4343 | Main passthrough → `libhip_intercept.so` (generated FWD stubs over full HIP surface). |
| `stubs_generated.c` | 7075 | Auto-generated dlsym forwarding stubs (~500 symbols). |
| `passthrough.c` | 439 | Cleaner function-table passthrough (core API). |
| `simple_passthrough.c` | 212 | Minimal RTLD_GLOBAL passthrough. |
| `hip_logging.c` | 600 | Logging interceptor → `libhip_logging.so`. |
| `hip_buffer_tracer.c` | 979 | Buffer-tracing interceptor → `libhip_buffer_tracer.so`. |
| `interceptors/passthrough_interceptor.c` | 32 | No-op interceptor → `libhip_noop.so`. |
| `interceptors/logging_interceptor.c` | 291 | Standalone logging interceptor. |
| `hip_function_table.h` | 363 | **The ABI contract**: `hip_function_table_t`, HIP types, interceptor interface. |
| `passthrough.map` / `passthrough_full.map` | — | Linker version scripts (core / full hip_4.2..7.2). |
| `add_logging.py` | — | Codegen that injects logging into stubs. |

### Interceptor ABI contract (must be preserved bit-for-bit in Rust)

```c
struct hip_function_table_t { uint32_t version; uint32_t struct_size; /* ~48 fn ptrs */ };

// Passthrough exports:
hip_function_table_t *hip_passthrough_get_real_table(void);
hip_function_table_t *hip_passthrough_get_active_table(void);

// Interceptor exports:
hip_function_table_t *hip_interceptor_init(hip_function_table_t *real_functions);
void                  hip_interceptor_shutdown(void);              // optional
pfn_hip_log_fn        hip_interceptor_get_log_fn(void);            // optional
```

Environment variables:
- `HIP_PASSTHROUGH_BACKEND_LIB` (**required**) — path to the real `libamdhip64.so`.
- `HIP_PASSTHROUGH_INTERCEPTOR` — optional interceptor `.so`.
- `HIP_LOG_FILE`, `HIP_LOG_LEVEL` (0..3) — logging interceptor.
- `HIP_TRACE_FILE`, `HIP_TRACE_LEVEL` (0..4), `HIP_TRACE_SYNC`, `HIP_TRACE_DUMP`,
  `HIP_TRACE_DUMP_MAX`, `HIP_TRACE_KERNEL_FILTER`, `HIP_TRACE_KERNEL_COUNT`,
  `HIP_TRACE_KERNEL_FULL_DUMP` — buffer tracer.

## 3. Decisions locked

- **Repo**: local git at `~/github/hrx-system-rs` (no remote yet).
- **IREE runtime**: FFI-wrap (`iree-sys`); port only `libhrx`.
- **LD_PRELOAD scope**: both paths; passthrough first.
- **Test backend**: real `libamdhip64.so` from a **TheRock nightly** installed in a
  venv (`~/github/therock-nightly-venv`, index `gfx120X-all` for gfx1201). The host
  has an RX 9070 XT (gfx1201), so device calls can run live; host-side calls and
  the logging/forwarding contract are exercised regardless of GPU state.

## 4. Test reality

There is **no existing automated test for the LD_PRELOAD path** in upstream — only
documented manual usage. The CTS (`libhrx/cts/`) is 15 Catch2 binaries / 68 cases
that `dlopen` `libhrx.so`; 12 require a GPU (`requires-gpu-amd` label), 3 are
CPU-only (`host_allocator`, `status`, `cxx_api`). So part of this effort is to
**create** an automated LD_PRELOAD test (golden trace/forward verification) that
becomes the conformance oracle for the Rust port.

## 5. Phases

### Phase 0 — Baseline: build + test the passthrough LD_PRELOAD path (C)  ← in progress
- Build the four passthrough `.so`s standalone with gcc. **DONE** — all four build
  clean (`scripts/build_c_baseline.sh`).
- Install TheRock nightly (gfx120X-all) into `~/github/therock-nightly-venv`; locate
  its `libamdhip64.so` for use as `HIP_PASSTHROUGH_BACKEND_LIB`.
- Write a small HIP smoke app + a harness that runs it under
  `LD_PRELOAD=libhip_intercept.so` with the logging interceptor, capturing the
  trace as a golden file. Define expected output modulo addresses/timestamps/PIDs.
- Deliverable: reproducible build+test, golden output committed under `tests/golden/`.

### Phase 1 — Rust port of the passthrough path
- Cargo workspace; crates:
  - `hip-function-table` — the `hip_function_table_t` struct + HIP types + interceptor
    ABI as `#[repr(C)]`. Single source of truth, layout-checked against the C header.
  - `hip-intercept` (`cdylib`) → `libhip_intercept.so` / `libamdhip64.so`: `#[no_mangle]
    extern "C"` exports for every intercepted symbol, `dlsym(RTLD_NEXT)`/backend dlopen,
    `ctor`-based init mirroring the C lifecycle, version-script parity.
  - `hip-logging`, `hip-buffer-tracer`, `hip-noop` (`cdylib`s) — interceptors exporting
    `hip_interceptor_init` etc.
- **Cross-ABI tests**: Rust interceptor + C passthrough, and C interceptor + Rust
  passthrough, must interoperate (proves ABI parity).
- **Differential tests**: same app under C vs Rust `libhip_intercept.so`; assert
  byte-identical trace modulo addresses. Port the manual usage into
  `tests/preload.rs` integration tests.

### Phase 2 — Public HRX C ABI + HIP-compat over FFI-wrapped IREE
- `iree-sys`: bindgen FFI over the IREE C runtime (built from `runtime/` or a ROCm/
  TheRock-provided IREE). Quantify the exact `iree_hal_streaming_*` surface libhrx uses.
- `hrx` crate: reproduce `hrx_runtime.h`'s 111-function C ABI via `#[no_mangle]` +
  cbindgen header generation, delegating to `iree-sys`. → `libhrx.so`.
- `hip-binding` crate: reproduce the 240-function HIP surface over the streaming HAL.
  → HRX `libamdhip64.so` (the product LD_PRELOAD path).

### Phase 3 — CTS port + packaging + CI
- Port the 15 CTS suites to Rust (custom harness mirroring the Catch2 fixture +
  `--hrx-library`/`--hrx-device` args + `requires-gpu-amd` labels). CPU-only suites
  run in CI; GPU suites gated on a gfx1201 runner.
- Reproduce the install layout (`libhrx.so`, `hrx-info`, `libamdhip64.so`) and the
  relocatable CTest tree. GitHub Actions mirroring `build_tools/ci_core_linux.py`.

## 6. Risks / open items
- IREE FFI surface depth (Phase 2): the streaming HAL is internal; bindgen over
  internal headers may be large. Mitigation: wrap only the functions libhrx calls.
- ABI exactness: HIP struct layouts (`hipDeviceProp_t`, etc.) are opaque in the
  passthrough header but concrete in real HIP — for forwarding we only need pointer
  pass-through, so Phase 1 is unaffected; Phase 2 needs real layouts.
- Full monolithic build (Phase 2) needs ROCm clang — available via TheRock dist
  (`~/github/TheRock/therock-build/dist/rocm`) and `/opt/rocm-7.2.0`.

## 7. Alternatives Considered

- **Port the entire repo including IREE to Rust.** Rejected: ~635K LOC of upstream
  IREE; effectively a separate multi-year project with no HRX-specific value.
- **Reimplement HIP semantics in Rust from scratch (no IREE).** Rejected: HRX's
  value *is* the IREE-backed implementation; a clean reimpl would diverge in
  behavior and lose the conformance target.
- **Rust-native API instead of exact C ABI.** Rejected for the shipped libraries:
  these must be drop-in `.so`s (`libamdhip64.so`, `libhrx.so`) with matching symbol
  versions, so the exported surface is fixed by ABI. Rust-native wrappers can be an
  additive convenience crate later.
- **Test against a hand-written mock HIP backend instead of real ROCm.** Considered;
  deferred. A mock is more deterministic for CI, but the user chose the real
  TheRock-nightly `libamdhip64.so` so the baseline reflects true HIP behavior. A mock
  backend may still be added later for hermetic CI.
