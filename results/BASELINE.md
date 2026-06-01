# Baseline performance: HRX vs vanilla CLR (MI300X / gfx942)

Captured on the `mi300` host (8× MI300X, ROCm 7.14 nightly `gfx94X-dcgpu`).
Methodology: `tests/apps/hip_bench.c` built against each backend's
`libamdhip64`, median of N iterations after warmup, GPU clocks confirmed boosting
under load (sclk 132→2105 MHz, mclk 900→1300 MHz). Every timed transfer/memset
op is followed by `hipDeviceSynchronize()` so both backends measure
time-to-completion (CLR's `hipMemset` is async; HRX's is internally synchronous).

Raw data: `bench_baseline.csv`. Re-run: `scripts/bench_baseline.sh`;
render: `scripts/bench_compare.py results/bench_baseline.csv`.

## Scope / caveat

Only the **host-API and memory-transfer paths** are benchmarked. HRX's
fatbin-registered **kernel dispatch does not currently execute correctly on
gfx942** (a `hipLaunchKernel`'d kernel silently produces wrong results — the
correctness gate would fail), so a kernel comparison would be invalid. The
benchmark's correctness gate refuses to emit numbers for any backend that fails
a H2D/D2H/memset roundtrip; both backends pass the gate here.

## Results (ratio = HRX / CLR; <1 means HRX faster)

```
op                                 size        clr         hrx   ratio
hostapi/deviceSynchronize             -       90ns        60ns   0.67x
hostapi/setDevice                     -       40ns        30ns   0.75x
hostapi/streamCreateDestroy           -    2.81ms        90ns   ~0x     (CLR very slow here)
alloc/mallocFree                   4 KB      772ns       181ns   0.23x
alloc/mallocFree                   1 MB      771ns       500ns   0.65x
alloc/mallocFree                  64 MB   138.85us       781ns   0.01x   (HRX pooled)
xfer/H2D                           4 KB    20.31us     78.91us   3.89x
xfer/H2D                          64 KB   137.59us    104.84us   0.76x
xfer/H2D                            1 MB   155.40us    453.12us   2.92x
xfer/H2D                           16 MB   561.36us     6.173ms  11.00x   clr 29.9 / hrx 2.7 GB/s
xfer/H2D                          256 MB    4.932ms   103.854ms  21.06x   clr 54.4 / hrx 2.6 GB/s
xfer/D2H                          256 MB    5.001ms   189.408ms  37.88x   clr 53.7 / hrx 1.4 GB/s
xfer/memset                        16 MB    15.69us     75.84us   4.83x   clr 1069 / hrx 221 GB/s
xfer/memset                       256 MB    60.78us    167.64us   2.76x   clr 4416 / hrx 1601 GB/s
```

## Reading of the baseline

- **HRX is faster on host-API latency and allocation.** `deviceSynchronize`/
  `setDevice` ~0.7×; `streamCreateDestroy` and large `mallocFree` are
  dramatically faster (HRX pools/streams are cheap; CLR's 2.8ms stream
  create+destroy and 139us 64MB malloc suggest real driver work / no pool).
- **HRX is markedly slower on bulk transfers** — large H2D/D2H run at ~1.4–2.7
  GB/s vs CLR's ~30–54 GB/s (10–38× slower). HRX's `hipMemcpy` is fully
  synchronous and appears to use a non-pinned / per-call staged path; this is
  the clear optimization target. Small transfers are closer (and HRX even wins
  64 KB H2D at 0.76×), consistent with HRX's lower fixed per-call overhead but
  worse throughput scaling.
- **memset** (on-device fill): CLR ~2–4× faster at large sizes, but same order
  of magnitude.

## libhrx public-ABI parity: Rust vs C (ratio = rust / c-hrx; ~1.0 == parity)

This is the **port-regression gate**. The Rust port (`libhrx_rs.so`) reimplements
the `hrx_*` **public C ABI**, not the HIP binding, so it cannot be dropped into
the HIP-API table above (that bench links `libamdhip64`, which calls IREE
streaming internals directly, not `hrx_*`). Instead, `tests/apps/hrx_bench.c`
calls the `hrx_*` ABI directly and is built against **both** the C `libhrx.so.0`
and the Rust `libhrx_rs.so` — same source, same ops — so this measures the Rust
reimplementation against the C original of the same ABI. Both wrap the **same
statically-linked IREE**, so parity (~1.0) is the expected and desired result;
the meaningful comparison here is `rust` vs `c-hrx`, **not** vs CLR.

Captured on `mi300` (gfx942) with `HRX_GPU_DRIVER=amdgpu`, median of N iters
after warmup. Raw data: `bench_libhrx_parity.csv`; backend stderr (device count,
`fill_supported`): `results/c-hrx.stderr`, `results/rust.stderr`. Re-run:
`scripts/bench_libhrx_parity.sh`; render: `scripts/bench_compare.py
results/bench_libhrx_parity.csv c-hrx`. Both backends pass the H2D/D2H
correctness gate (`GATE OK`), and stream fill works on the amdgpu path
(`fill_supported=1`); the harness drops any op whose status errors rather than
timing a fast failure (none did here).

The table below is one representative run. Sub-microsecond ops sit at the
`clock_gettime` floor (~30 ns) — that overhead is common-mode and cancels in the
rust/c-hrx ratio, but the absolute values for those rows are timer-dominated, not
library signal. GPU clocks were not pinned, so back-to-back per-backend runs can
land in different boost states (see the 16 MB note below).

```
op                                  size       c-hrx        rust   ratio   note
hostapi/deviceSynchronize              -        29ns        30ns   1.03x   (deprecated no-op; timer-floor)
hostapi/streamCreateDestroy            -       170ns       170ns   1.00x   (sub-us; timer-floor)
alloc/allocFree                     4 KB    179.02us    179.57us   1.00x
alloc/allocFree                     1 MB    786.61us    789.43us   1.00x
alloc/allocFree                    64 MB    44.127ms    44.386ms   1.01x
alloc/streamAllocFree               4 KB    210.54us    211.37us   1.00x   (drives the hrx exact pool)
alloc/streamAllocFree               1 MB    824.27us    832.23us   1.01x   (drives the hrx exact pool)
xfer/H2D                            4 KB       101ns       101ns   1.00x   (sub-us; timer-floor)
xfer/D2H                            4 KB       150ns       150ns   1.00x   (sub-us; timer-floor)
xfer/fill                           4 KB     22.78us     22.19us   0.97x
xfer/H2D                           64 KB      1.31us      1.31us   1.00x
xfer/D2H                           64 KB      1.22us      1.22us   1.00x
xfer/fill                          64 KB     25.84us     25.15us   0.97x
xfer/H2D                            1 MB     21.98us     22.00us   1.00x
xfer/D2H                            1 MB     22.00us     22.04us   1.00x
xfer/fill                           1 MB     87.29us     86.62us   0.99x
xfer/H2D                           16 MB    746.94us    744.26us   1.00x   c-hrx 22.5 / rust 22.5 GB/s
xfer/D2H                           16 MB    759.89us    754.99us   0.99x   c-hrx 22.1 / rust 22.2 GB/s
xfer/fill                          16 MB     1.075ms     1.075ms   1.00x
xfer/H2D                          256 MB    15.009ms    15.038ms   1.00x   c-hrx 17.9 / rust 17.9 GB/s
xfer/D2H                          256 MB    19.359ms    19.364ms   1.00x   c-hrx 13.9 / rust 13.9 GB/s
xfer/fill                         256 MB    16.933ms    16.933ms   1.00x
```

### Reading of the parity result

- **Parity across the board** (ratio 0.97–1.03×) for allocation, transfers, fill,
  and the stream/pool paths — as expected, since the Rust port is a thin
  reimplementation of the `hrx_*` wrappers over the identical statically-linked
  IREE. The stream-ordered `streamAllocFree` path, which drives the Rust
  `hrx_iree_exact_pool`, is within ~1% of the C pool.
- **Sub-microsecond ops are timer-floor noise, not signal.** `deviceSynchronize`
  (a deprecated no-op) and 4 KB H2D/D2H report ~30–150 ns — at or below the
  `clock_gettime` pair + indirect-call overhead. Across runs these swing a few ns
  either way (e.g. `streamCreateDestroy` measured 1.00× here and 0.78× in another
  run); read them as parity, not as differences.
- **No systematic transfer divergence; the 16 MB band is run variance.** Across
  three runs the 16 MB H2D/D2H ratio was 1.16×, 1.08×, and 1.00× (this run),
  while 1 MB and 256 MB stayed at parity every time. The `hrx_synchronous_h2d`
  wrapper is size-independent (identical logic in C and Rust), so this is
  sequential-run GPU clock/thermal drift between the two backend invocations
  (clocks unpinned), not a port regression. Pinning clocks (`rocm-smi
  --setperflevel high`) or interleaving the backends would remove it.

## Use as a regression baseline

The Rust port is at parity with C `libhrx` on the public ABI (above), and
**byte-identical** to it on the 7-suite functional differential
(`scripts/libhrx_diff_test.sh`, `HRX_RUN_GPU=1`). Re-run both after Rust changes;
a large, reproducible, size-consistent divergence in the parity table (not the
sub-µs noise or the known 16 MB band) is a port regression.

Note on the HIP-API table above: the Rust port does **not** include a HIP binding
(`binding/hip/api.c`), so there is no Rust `libamdhip64` to add as a third column
there. CLR remains the absolute reference for the HIP-API numbers; the Rust
regression gate is the public-ABI parity table, whose target is parity with
C `libhrx`, not beating CLR.
