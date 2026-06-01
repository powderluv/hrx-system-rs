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

## Use as a regression baseline

When the Rust `libhrx` / HIP-binding port lands, build a third backend binary
from the same `hip_bench.c` and add a `rust` column. The goal for the Rust port
is **parity with the C HRX numbers** (it wraps the same IREE streaming layer);
any large divergence is a port regression. CLR remains the absolute reference.
