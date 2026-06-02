# HRX Rust Port — Safety Remediation Plan

Status: Draft, 2026-06-01.
Responds to: [`hrx-rust-port-safety-assessment.md`](../hrx-rust-port-safety-assessment.md).

## TL;DR

The assessment is correct. `crates/hrx-libhrx` is a faithful *transliteration*: raw
pointer handles are the internal object model, lifetimes are manual `libc`
alloc/free + hand-rolled refcounts, ~167 IREE FFI call sites are spread across 13
of 16 modules, there are **zero `Drop` impls**, and **8 exported functions are
safe Rust `extern "C"` that dereference caller pointers** (genuine soundness
defects). It proved ABI + performance parity over the same IREE; it did not move
the memory-safety story.

This plan turns the transliteration into a Rust-owned object model behind the
same C ABI, brings it into an equal rigor envelope (sanitizers, fuzzing, Miri,
CTS, ABI gates, CI), and — because the explicit requirement is *do not regress
performance* — gates every phase on the existing byte-identical differential
**and** the `rust / c-hrx` parity bench. Two recon findings make this tractable:

1. **All 11 public handle structs are opaque-only to C** (`typedef struct
   hrx_X_s *hrx_X_t;`); the only internals-punning consumer in the C tree
   (`streaming/hrx_bridge.h`) is not linked by the Rust port. So they can become
   `Arc`/`Box`/`Drop`-owned, non-`repr(C)`, **with no observable ABI change**.
2. **The libhrx CTS is `dlopen` + `--hrx-library <path>`** — 14 of 15 suites run
   against `libhrx_rs.so` unmodified. The conformance envelope is reusable as-is.

## Goals

- Encode lifecycle and state invariants in Rust types so whole bug classes
  (double free, use-after-free, retain/release imbalance, map/unmap imbalance,
  use-after-parent-shutdown) become **unrepresentable**, not convention.
- Confine `unsafe` to a small, audited FFI/handle-boundary surface; make the bulk
  of `hrx-libhrx` safe Rust.
- Fix the unsound safe-API defects.
- Bring the port inside an equal safety-tooling envelope (the assessment's core
  "don't become an island" point).
- **Preserve the C ABI exactly** (114 `hrx_*` symbols, byte-identical behavior).
- **Preserve performance** (rust/c-hrx parity, gated every phase).

## Non-goals

- Reimplementing IREE (it stays a reused prebuilt static substrate).
- Porting the HIP binding or the streaming layer (out of scope, as today).
- Changing the public C ABI surface or semantics.
- A from-scratch rewrite (we keep the proven parity and migrate incrementally
  behind the differential).

## Design principles

1. **The public C ABI stays raw opaque pointers.** Only the *internal* object
   model changes. The unsafe operation is confined to converting a C handle to a
   checked internal reference, in one audited place.
2. **Prove, don't claim.** Every phase must pass the 7-suite differential
   (byte-identical) + the libhrx CTS + the parity bench (≤ threshold) before it
   merges. Perf is measured, not argued.
3. **Match C teardown order.** Ownership replaces manual per-call accounting, but
   `Drop` impls must release underlying IREE/parent handles in the *same order*
   the C last-ref path uses, so observable behavior is unchanged.
4. **Shrink unsafe monotonically.** `cargo-geiger` is a ratchet; the unsafe count
   may not grow.

---

## Part A — Internal architecture: owned objects behind the same ABI

Addresses assessment concerns: raw opaque handles as object model; manual
alloc/free; informal retain/release; unsafe FFI thunks throughout; unsound safe
APIs; state-as-convention; FFI spread through business logic.

### A0. The enabling fact (ABI partition)

Recon confirms a clean split (full table in the recon notes):

- **Must stay `#[repr(C)]`** (C reads the bytes / crosses the ABI by value): the
  exported global `hrx_host_allocator_system_value` + `HrxHostAllocator`; the
  by-value/by-pointer param structs (`HrxBufferParams`, `HrxSemaphoreList`,
  `HrxBufferRef`, `HrxDispatchConfig`, `HrxTimelinePoint`,
  `HrxExecutableExportInfo`, `HrxHostCallFn`, the `HrxStatusCode` enum); the
  IREE-ABI mirror structs in `pool.rs` (`HrxExactPool` + the `IreeHal*` vtable/
  reservation structs — IREE dereferences these); and `HrxStatusS` (shares libc
  malloc/free discipline with the C status object).
- **Free to become Rust-owned** (opaque to C, non-`repr(C)`, reorderable):
  `HrxDeviceS`, `HrxAllocatorInline`, `HrxSemaphoreS`, `HrxStreamS`,
  `HrxBufferS`, `HrxModuleS`, `HrxFunctionS`, `HrxValueListS`, `HrxFenceS`,
  `HrxBufferViewS`, `HrxExecutableS`.

This is what makes the refactor ABI-safe.

### A1. Safe HAL wrapper crate (`iree-hal`)

Introduce a new crate `iree-hal` between `iree-sys` (raw decls only) and
`hrx-libhrx`. It provides **zero-cost RAII newtypes** over the 21 IREE object
types the port manipulates (recon enumerated them with their retain/release/
create FFI):

```rust
// thin, #[inline], no added indirection — the wrapper IS the raw pointer.
#[repr(transparent)]
pub struct HalBuffer(NonNull<iree_sys::iree_hal_buffer_t>);

impl Clone for HalBuffer {            // == iree_hal_buffer_retain
    fn clone(&self) -> Self { unsafe { iree_sys::iree_hal_buffer_retain(self.0.as_ptr()); } Self(self.0) }
}
impl Drop for HalBuffer {             // == iree_hal_buffer_release
    fn drop(&mut self) { unsafe { iree_sys::iree_hal_buffer_release(self.0.as_ptr()); } }
}
impl HalBuffer {
    pub fn byte_length(&self) -> u64 { unsafe { iree_sys::iree_hal_buffer_byte_length(self.0.as_ptr()) } }
    // ... one audited unsafe block per HAL call, with a // SAFETY: note.
}
```

- ~14 types follow the symmetric `Clone`(retain)/`Drop`(release)/`create`
  pattern; ~7 (driver, vm_instance, async_*, task_executor, command_buffer,
  executable_loader) are release-only → move-only owned handles (no `Clone`).
- This is the single place `unsafe` IREE calls live. High-level `hrx-libhrx`
  modules then call **safe** methods; the ~167 scattered FFI sites collapse to
  the wrapper crate.
- **Miri payoff:** define the wrapper surface as a `trait Hal` so the pure-Rust
  object model can be unit-tested against an in-memory fake under Miri (Miri
  cannot execute the real FFI — this is the forcing function the recon flagged).

### A2. Owned object model + handle boundary

Each opaque handle becomes a thin thunk over an `Arc`-owned Rust object:

```rust
// C ABI type stays opaque; never field-read by C.
pub struct hrx_buffer_s { _private: [u8; 0] }
pub type hrx_buffer_t = *mut hrx_buffer_s;

struct Buffer {                 // Rust-owned; not repr(C)
    hal: HalBuffer,             // Drop releases the IREE buffer
    pool: Option<HalPool>,      // Drop releases the pool
    device: Arc<Device>,        // keeps the device alive for the buffer's life
    map: Mutex<BufferMapState>, // see A4 (or a cheaper exclusive cell; see B2)
    size: usize,
    mem_type: u32,
}
```

- **Handle = `Arc::into_raw(arc) as hrx_buffer_t`.** `hrx_buffer_retain` =
  `Arc::increment_strong_count(h)`; `hrx_buffer_release` = `Arc::from_raw(h)`
  drop. The last drop runs `Buffer::drop`, which tears down `hal`/`pool`/`device`
  in the **same order** as C's `hrx_buffer_release`. (`Arc::into_raw` returns the
  data pointer, so the handle is a single deref to the object — no extra
  indirection.)
- **One audited unsafe boundary**, macro-generated per type, converts a raw
  handle to `&Object` (borrow) or `ManuallyDrop<Arc<Object>>` (for
  retain/release). This is the only place that trusts a caller pointer.
- **Refcount accounting changes from per-call to structural.** C re-retains the
  device on every buffer-retain and re-releases on every buffer-release (nets to
  zero); the owned model holds one `Arc<Device>` for the buffer's lifetime and
  releases it exactly when the buffer dies. Net object lifetimes are identical;
  the *sequence* of underlying IREE release calls differs, so the differential +
  CTS must confirm no observable divergence (Risk R1).

### A3. Soundness fixes (immediate, Phase 0)

- Mark the 8 unsound exports `unsafe extern "C"`: `hrx_status_code`,
  `hrx_make_status`, `hrx_host_allocator_{malloc, malloc_uninitialized, realloc,
  realloc_aligned, malloc_aligned, clone}` (+ the 2 borderline `free` fns, +
  tighten `hrx_runtime_version` to also reject misaligned). The C ABI symbol is
  unchanged; only Rust callers must use `unsafe`. **Zero runtime cost.**
- Add `#![deny(unsafe_op_in_unsafe_fn)]` to every crate root (forces an explicit
  `unsafe {}` + `// SAFETY:` per op; mechanical first-pass churn).
- The `rlib` crate-type is dead (no consumer). Until there is a real Rust
  consumer, drop `hrx-libhrx` to `["cdylib"]` so there is no safe-Rust-callable
  surface at all (belt-and-suspenders alongside the `unsafe` markings).

### A4. State-as-types

- `enum BufferMapState { Unmapped, Mapped(BufferMapping) }`; `BufferMapping`
  releases the IREE mapping in `Drop`. Re-mapping requires dropping the previous
  mapping; double-map / unmap-balance bugs become unrepresentable. (Perf note in
  B2 — map/unmap is not on the bulk-transfer hot path.)
- Stream timeline / pending-command-buffer state moves behind a small state
  type so "flush with orphaned timepoint" (a known C hang) is structurally
  discouraged.

### A5. Error model

- Internal code returns `Result<T, HrxError>`. Only the thin C-ABI thunks convert
  to `hrx_status_t`. Rust APIs never expose `hrx_status_t` as a safe pointer
  type. IREE⇄HRX status conversion is centralized in one audited module
  (already mostly true via `hrx_status_from_iree`).

### A6. Send/Sync hygiene

- Replace the 5 ad-hoc `unsafe impl Send/Sync` (4 in `runtime.rs`, 1 in
  `pool.rs`) with per-wrapper `unsafe impl` carrying a `// SAFETY:` proof, or
  remove them where the owned types are auto-`Send`/`Sync`. The global `Mutex`
  stays; what changes is that the things inside it are owned wrappers, not raw
  pointers.

---

## Part B — Performance preservation (explicit requirement)

Addresses the user's requirement and the assessment's "performance gates" rubric
row.

### B1. Why it stays at parity

- The hot paths (transfers, alloc, dispatch, queue submit) are dominated by the
  IREE call; the wrapper is a single deref + the same FFI call. The parity bench
  already measured the wrapper overhead at **≈ 0% vs C** across 4 KB–256 MB.
- `Arc` retain/release are the **same atomic inc/dec** as today's `AtomicI32`
  refcount. `Arc::into_raw` hands back the data pointer, so a handle is the same
  single deref — no added indirection.
- The `iree-hal` newtypes are `#[repr(transparent)]` + `#[inline]`; they compile
  to the identical machine code as the current inline FFI calls.

### B2. Pitfalls that *would* cost performance — explicitly avoided

- **No hot-path handle registry / generation lookup.** A global table check per
  call would add a hashmap hit to every op. We do **not** do that in release;
  the handle→object conversion stays a pointer cast. (Use-after-free *testing* is
  handled by B3, off in release.)
- **No blanket `Mutex` on per-object exclusive state.** The C handle model is
  single-threaded-per-handle; wrapping bulk-transfer state in a lock would add
  contention. Per-object state that C accesses exclusively uses the cheapest
  representation consistent with that contract; only genuinely-shared state
  (globals) keeps the existing `Mutex`. Buffer map-state is touched only on
  map/unmap (cold), so its `Mutex`/state-enum cost is irrelevant to throughput.
- **Match C `Drop` order exactly** — not just for correctness but because IREE
  release ordering can affect pool/semaphore reuse timing.
- **Watch added bounds checks / `Arc` weak-count (16 B/alloc)** — both negligible
  and off the hot path; confirm with the bench, don't assume.

### B3. Optional debug-only hardening

A registry/generation scheme that detects use-after-free/double-free is
**compiled out in release** (`#[cfg(feature = "handle-guard")]`) and enabled only
in the debug/ASAN test lane. This gets the safety-testing benefit with zero
release-perf cost.

### B4. Perf is a hard gate

Extend `scripts/bench_libhrx_parity.sh` with a ratio assertion (fail if any
throughput-bound op's `rust/c-hrx` median ratio exceeds ~1.05, with sub-µs ops
exempt as documented noise; gate on median over multiple runs). **Every Part-A
phase must pass this on the MI300 before merge.** This is the concrete answer to
"ensure performance is preserved": we re-measure parity at each step and block on
regression.

---

## Part C — Rigor envelope (stop being an island)

Addresses the assessment's strongest point: the C runtime has sanitizers,
fuzzers, CTS, CI, and review policy; the Rust port currently has none of it.

### C1. Conformance — reuse the existing CTS (highest-value, near-free)

- Run the **libhrx CTS against `libhrx_rs.so`**: `ctest` with
  `--hrx-library /path/to/libhrx_rs.so` (or `HRX_LIBHRX_PATH`). 14/15 suites are
  pure `dlopen` and need no test changes; `cxx_api` links the C lib directly and
  is deferred. Suites: host_allocator, lifecycle, device, allocator, semaphore,
  stream, stream_ops, memory, transfer, executable, queue_ops, status, refcount,
  virtual_memory.
- Keep the existing 7-suite byte-identical differential + `hrx_bench` as the
  fast inner loop.

### C2. Sanitizers (mixed C/Rust lanes)

- **ASAN (primary gate).** Build a second IREE archive set with
  `-DIREE_ENABLE_ASAN=ON` (`$HRX_BUILD_DIR_ASAN`); build the Rust cdylib with
  `RUSTFLAGS=-Zsanitizer=address` + `-Zbuild-std` on the pinned nightly; run the
  differential + a handle lifecycle stress test under ASAN. Catches UAF/double-
  free/OOB across the FFI boundary. Constraint: clang LLVM version must match
  nightly's compiler-rt (pin both; assert in CI).
- **UBSAN** (`IREE_ENABLE_UBSAN=ON` + Rust `-Zsanitizer`) — catches by-value
  struct ABI mismatches and integer UB; C side carries most coverage.
- **TSAN — best-effort** (the global `Mutex`/Send/Sync claims + a concurrency
  stress test). Caveat: the amdgpu HAL `dlopen`s HSA, an uninstrumented blind
  spot; treat as advisory, not a hard gate.
- **MSAN — out of scope** (requires every dep incl. libc and dlopen'd HSA to be
  MSAN-built); document the decision.

### C3. Fuzzing

`cargo-fuzz` (libFuzzer, nightly) targets on the pure-Rust marshaling/validation
surfaces — mirroring IREE's 50 existing fuzzers: status decode, `value_list`
ops, `buffer_view` construction with adversarial shapes, dispatch
config/binding decode, and a **stateful handle-lifecycle fuzzer** (random
alloc/retain/release/map/unmap sequences). GPU-free targets first (run on
ordinary x86 CI).

### C4. Miri

`cargo +nightly miri test` over the pure-Rust modules (`status`,
`host_allocator`, `value_list`, `common`) today, expanding to the object model
once A1's `trait Hal` lets it run against an in-memory fake. Miri cannot cross
the real FFI — this limitation is exactly why the HAL trait abstraction (A1) is
worth it.

### C5. Static analysis / lint / review policy

- `#![deny(unsafe_op_in_unsafe_fn)]` (A3), `clippy::pedantic` (start advisory →
  `-D warnings` once clean), a `// SAFETY:` comment on every `unsafe` block and
  `unsafe impl`.
- `cargo-geiger` as a tracked ratchet (advisory; v0.13.0 can choke on
  workspaces, so non-blocking) — the unsafe surface (`hrx-libhrx` 159) must
  trend **down**, not up.
- An `unsafe`-code review checklist in `CONTRIBUTING`: every unsafe block names
  its invariant; every ABI type names its layout source.

### C6. ABI / layout drift gates (cheap, GPU-free)

- **Exported-symbol parity test**: `nm -D --defined-only libhrx_rs.so | grep ' T
  hrx_'` must equal the C `libhrx.so.0` `hrx_*` set (extend the existing
  `tests/golden/` + `build_c_baseline.sh` symbol counter). Fails on any
  missing/extra symbol.
- **By-value struct ABI test**: formalize the `abi_probe` methodology used during
  porting — compile a tiny C probe against the real IREE headers and assert
  `sizeof`/`offsetof` match every `#[repr(C)]` model (`iree_hal_buffer_params_t`,
  dispatch config, semaphore list, the pool/vtable structs, …). Catches IREE ABI
  drift (the class of bug that the `min_alignment` fix already exposed once).

### C7. CI (the thing that makes it not-an-island)

A GitHub Actions workflow on `powderluv/hrx-system-rs` (none exists today):

- **x86 lanes (every PR, ordinary runners):** build, `cargo test`,
  `clippy -D warnings`, `deny(unsafe_op_in_unsafe_fn)`, symbol-parity test, ABI
  struct test, Miri on pure-Rust crates, GPU-free fuzz smoke, `cargo-geiger`
  (advisory).
- **GPU lanes (self-hosted MI300 runner or `workflow_dispatch`):** the 7-suite
  differential, the libhrx CTS, the ASAN/UBSAN mixed lanes, and the perf parity
  gate (B4).
- **Pin `rust-toolchain.toml`** (stable for the x86 gates; the already-installed
  `nightly-2026-04-03` for sanitizers/Miri/fuzz) so results are reproducible.

### C8. Cohesion with the C tree

- Record the IREE archive provenance/version that `iree-sys` links and add an
  ABI-drift gate (C6) so the Rust port can't silently diverge from the C IREE it
  reuses.
- Parallel recommendation (C-side, not our deliverable): the assessment notes
  `libhrx` historically had less rigor than core IREE; the right baseline is to
  also run the C `libhrx` under the IREE ASAN/CTS envelope so the safety/perf
  comparison is apples-to-apples.

---

## Part D — Phased rollout

Each phase merges only after: 7-suite differential byte-identical **and** libhrx
CTS green **and** parity bench ≤ threshold on MI300. No phase is "done" on
`cargo test` alone.

| Phase | Scope | Risk | Perf risk | Exit criteria |
|---|---|---|---|---|
| **0. Scaffolding + soundness** | A3 (mark 8 fns `unsafe`, drop dead rlib, `deny(unsafe_op_in_unsafe_fn)`); C6 symbol + ABI tests; C7 CI skeleton; B4 perf threshold; clippy baseline. No architecture change. | low | **none** (no codegen change) | CI green; differential + bench unchanged. |
| **1. Safe HAL wrapper** | A1 `iree-hal` crate (RAII over 21 IREE types) + `trait Hal`; migrate leaf modules (`semaphore`, `fence`, `buffer_view`, `host_allocator`) off raw FFI. | med | low (inline newtypes) | unsafe count drops; Miri runs on leaf modules; bench parity. |
| **2. Owned object model** | A2 per handle, simplest→hardest: semaphore, fence, buffer_view → buffer, stream, device → executable, module, value_list. `Arc`/`Drop`; handle-boundary module. `pool.rs` stays `repr(C)` (IREE-bound) but gets a safe wrapper. | high | low (Arc == atomic refcount) | differential + CTS byte-identical; bench parity; **R1 confirmed** (refcount-timing equivalence). |
| **3. State + error types** | A4 buffer map-state enum + `Drop`-unmap; A5 internal `Result`. | med | none (cold paths) | map/unmap balance enforced by types; differential green. |
| **4. Full envelope** | C2 ASAN/UBSAN lanes; C3 fuzz targets incl. stateful lifecycle fuzzer; C4 Miri on the object model; C5 geiger ratchet; TSAN best-effort. | med | n/a (test infra) | ASAN-clean differential; fuzzers run; CI matrix green. |

Phases 0 and 1 deliver most of the de-risking (soundness fixed, FFI confined,
CI + gates live) at near-zero perf risk. Phases 2–3 are where bug classes
become unrepresentable. Each is independently shippable and reversible.

---

## Alternatives Considered

- **Keep `hrx-system-rs` as a parity oracle only; invest rigor in the C `libhrx`
  instead** (the assessment's literal practical recommendation). Legitimate and
  cheaper, and we adopt half of it (C8: harden the C side too). Rejected as the
  *sole* path because it concedes that the Rust port stays a non-safety artifact;
  the user asked to *address* the safety concerns, and the recon shows a real
  re-foundation is ABI- and perf-safe, so it's worth doing.

- **Full ground-up safe rewrite (re-foundation).** Cleanest end state, but throws
  away the proven 114/114 parity, has no incremental gate, and is high-risk.
  Rejected in favor of incremental migration *behind the existing differential +
  perf gates* — same end architecture, continuously validated.

- **Minimal fix: mark the 8 fns `unsafe`, add CI/sanitizers, keep raw-pointer
  internals.** Cheap; fixes soundness + the island problem. Rejected as the
  endpoint because it leaves "raw handles as the object model" (the assessment's
  central concern) untouched — but it *is* adopted as Phase 0/4 (the cheap wins
  ship first).

- **Hot-path handle registry / generational indices for use-after-free safety.**
  Real UAF protection, but a per-call table lookup regresses the parity we must
  preserve. Rejected for release; adopted as an optional debug/ASAN-only feature
  (B3).

- **Wrap all per-object state in `Mutex`/`RwLock` for thread-safety.** Matches the
  assessment's "better shape" literally, but adds lock cost the C model doesn't
  pay (handles are single-threaded-per-handle). Rejected for exclusive per-object
  state; kept only for genuinely shared globals.

- **Generate the whole crate from the C source.** Would re-derive a
  transliteration — the exact thing we're trying to leave. Rejected.

## Decision rubric (assessment's table, with target answers)

| Question | Plan's answer |
|---|---|
| Did raw C handles stop being the internal object model? | **Yes** — A2: `Arc`-owned objects behind opaque thunks. |
| Did manual refcount/free move into `Drop`/owned wrappers? | **Yes** — A1/A2: `Drop`-backed `iree-hal` + `Arc` handles; libc free removed for owned types. |
| Are unsafe blocks localized to FFI/device/loader boundaries? | **Yes** — A1 confines IREE FFI to `iree-hal`; A2 confines handle conversion to one module. |
| Are Rust-callable APIs sound? | **Yes** — A3: the 8 unsound fns become `unsafe`; dead rlib dropped. |
| Did the port encode state transitions as types? | **Yes** — A4: map-state enum + `Drop`-unmap. |
| Did fuzz/tests target the new boundary? | **Yes** — C3 stateful lifecycle fuzzer + C1 CTS + C4 Miri. |
| Did the build remain cohesive? | **Yes** — C6/C8 ABI-drift + provenance gates; C7 CI. |
| Did performance gates pass? | **Required every phase** — B4 parity-bench gate. |
| Did the Rust code enter the same sanitizer/fuzzer/CTS culture as C? | **Yes** — C1–C7 ASAN/UBSAN/fuzz/CTS/CI. |

## Risks & open questions

- **R1 (refcount-timing equivalence):** structural ownership changes the
  *sequence* of underlying IREE retain/release vs C's per-call accounting. Net
  lifetimes are equal, but a CTS/differential test could observe a difference
  (e.g. an object freed one call earlier/later). Mitigation: `Drop` order mirrors
  C; the differential + CTS are the gate; investigate any divergence before
  proceeding.
- **R2 (sanitizer build matching):** ASAN requires the IREE archives rebuilt with
  `IREE_ENABLE_ASAN` and a clang LLVM version matching nightly's compiler-rt.
  Mitigation: pin both; CI asserts the versions; start with a CPU-HAL ASAN run
  (no GPU) to de-risk before the MI300 lane.
- **R3 (MI300 runner availability):** the GPU gates (CTS, ASAN-over-dispatch,
  perf) need a self-hosted MI300 runner. Until one exists, run them as a
  documented manual/`workflow_dispatch` gate (as the current bench already is).
- **R4 (`HrxStatusS`/host-allocator stay `repr(C)`+libc):** these remain
  C-shaped by ABI necessity; they're the residual accepted-unsafe zone and must
  be documented as such (the assessment's "remaining unsafe zones explicitly
  accepted").
- **R5 (`pool.rs` stays `repr(C)`):** the exact pool is ABI-bound to IREE's
  vtable; it gets a safe wrapper but keeps its layout. Accepted, documented.

## Progress

**Phase 0 (in progress, 2026-06-01).** Landed and validated on MI300 (gfx942) —
7-suite differential byte-identical, symbol parity 115/115, ABI layout matches:

- [x] Soundness: the unsound safe `extern "C"` exports that deref caller
  pointers are now `unsafe` — `hrx_status_code`, `hrx_make_status`, the 8
  `hrx_host_allocator_*`, and `hrx_runtime_version` (the only remaining safe
  exports are the 4 pointer-free cpu/gpu init/shutdown fns). ABI unchanged.
  *(Deviation from A3: kept the `rlib` — marking the fns `unsafe` already makes
  it sound, and the `rlib` is needed for the later Miri/unit-test work.)*
- [x] ABI gates: compile-time `#[repr(C)]` layout asserts
  (`crates/iree-sys/src/abi_layout.rs` + pool structs in `pool.rs`),
  `scripts/check_abi_layout.sh` (re-probes IREE headers),
  `scripts/check_symbol_parity.sh` (rust vs C `hrx_*` set).
- [x] `rust-toolchain.toml` pin; CI skeleton (`.github/workflows/checks.yml`
  stock lanes, `gpu.yml` self-hosted MI300 lane). Note: `build.rs` requires the
  prebuilt archives, so iree-sys/hrx-libhrx build+gates are self-hosted.
- [x] Perf-gate threshold (B4): `scripts/bench_gate.py` (geomean ≤ 1.05 +
  per-op ≤ 1.25, timer-floor ops exempt) wired as the exit gate of
  `bench_libhrx_parity.sh`. Validated: PASS on the parity CSV (geomean 0.997),
  FAIL on a synthetic 30% regression. The loose per-op ceiling tolerates the
  known 16 MB clock-drift band.
- [ ] `#![deny(unsafe_op_in_unsafe_fn)]` crate-wide — deferred to land with the
  module refactors (Phases 1–2), to avoid churning code about to be rewritten.

**Phase 1 (in progress, 2026-06-01).** Foundation landed and validated on MI300
(7-suite differential byte-identical incl. the `fence` suite; perf gate PASS,
geomean 1.002):

- [x] `crates/iree-hal`: the safe-RAII wrapper crate, `#![forbid(unsafe_op_in_unsafe_fn)]`.
  `HalFence` is a `#[repr(transparent)]` newtype — `Drop` = `iree_hal_fence_release`,
  `Clone` = retain, constructors return the owned wrapper. Grown one IREE type at
  a time as modules migrate.
- [x] `crates/hrx-libhrx/src/handle.rs`: the single audited handle boundary —
  opaque `hrx_*_t` = `Arc<T>` data pointer; `retain`/`release` are `Arc` refcount
  ops (same atomics as the old manual refcount); `Drop` runs teardown once.
- [x] `fence.rs` migrated as the proof: raw `*mut HrxFenceS` + `libc::calloc`/`free`
  + manual `iree_hal_fence_release` accounting → `Arc`-owned `HrxFenceS { hal: HalFence }`
  with `Drop`-based release and **no direct IREE FFI in the module**. R1
  (refcount-timing equivalence) confirmed by the byte-identical `fence` suite.
- [ ] Remaining leaf modules (`semaphore`, `buffer_view`) + then Phase 2
  (`buffer`/`stream`/`device`) follow the same validated pattern. `Hal` trait /
  Miri deferred to when Miri-on-object-model is wired (Phase 4).

## Bottom line

The honest status stays "successful compatibility and parity port" — not "the
runtime is now memory-safe." This plan changes that, concretely and in shippable
increments, while keeping the one property we already proved: **performance
parity with C, measured at every step.** The deliverable of each phase is not "it
is in Rust," but: which unsafe boundary was isolated, which invariant a type now
encodes, which bug class is no longer representable, and which sanitizer / fuzz /
CTS / perf gate passed.
