#!/usr/bin/env python3
"""Performance-parity gate for the Rust libhrx port.

Reads a bench CSV (backends c-hrx + rust) and fails if the Rust port has
regressed against C beyond tolerance. Used after scripts/bench_libhrx_parity.sh
so a safety-hardening change that silently costs performance can't merge.

Two checks over the *throughput-bound* ops (sub-microsecond ops are exempt: at
the clock_gettime floor their ratio is timer noise, not signal):
  - geomean(rust/c-hrx) <= GEOMEAN_MAX  (catches systematic regressions)
  - max per-op rust/c-hrx <= PEROP_MAX  (catches one op blowing up)

The per-op ceiling is deliberately loose enough to tolerate the known 16 MB
transfer band, which drifts ~1.0-1.16x run-to-run from unpinned GPU clocks (see
results/BASELINE.md), not from the port.

Usage: bench_gate.py <csv> [baseline_backend=c-hrx] [target_backend=rust]
Env:   BENCH_FLOOR_NS (default 5000), BENCH_GEOMEAN_MAX (1.05), BENCH_PEROP_MAX (1.25)
"""
import csv
import math
import os
import sys


def main() -> int:
    path = sys.argv[1]
    base = sys.argv[2] if len(sys.argv) > 2 else "c-hrx"
    targ = sys.argv[3] if len(sys.argv) > 3 else "rust"
    floor_ns = float(os.environ.get("BENCH_FLOOR_NS", "5000"))
    geomean_max = float(os.environ.get("BENCH_GEOMEAN_MAX", "1.05"))
    perop_max = float(os.environ.get("BENCH_PEROP_MAX", "1.25"))

    med = {}  # (cat,name,bytes) -> {backend: median_ns}
    for r in csv.DictReader(open(path)):
        med.setdefault((r["category"], r["name"], int(r["bytes"])), {})[r["backend"]] = float(r["median_ns"])

    ratios = []
    worst = None
    exempt = 0
    for key, m in sorted(med.items()):
        if base not in m or targ not in m:
            continue
        b, t = m[base], m[targ]
        if b < floor_ns:  # timer-floor op: ratio is noise
            exempt += 1
            continue
        ratio = t / b
        ratios.append(ratio)
        if worst is None or ratio > worst[0]:
            worst = (ratio, key)

    if not ratios:
        print(f"perf-gate: no throughput-bound ops found for {targ} vs {base} in {path}")
        return 2

    geomean = math.exp(sum(math.log(x) for x in ratios) / len(ratios))
    wr, wk = worst
    print(f"perf-gate: {targ} vs {base} over {len(ratios)} throughput-bound ops "
          f"({exempt} sub-floor exempt)")
    print(f"  geomean ratio = {geomean:.3f}  (max {geomean_max})")
    print(f"  worst op      = {wr:.3f}x  {wk[0]}/{wk[1]} {wk[2]}B  (max {perop_max})")

    fail = False
    if geomean > geomean_max:
        print(f"  FAIL: geomean {geomean:.3f} > {geomean_max}"); fail = True
    if wr > perop_max:
        print(f"  FAIL: worst op {wr:.3f}x > {perop_max}"); fail = True
    if fail:
        print("PERF GATE FAILED: Rust port regressed vs C beyond tolerance")
        return 1
    print("PASS: performance within parity tolerance")
    return 0


if __name__ == "__main__":
    sys.exit(main())
