#!/usr/bin/env python3
"""Render a side-by-side comparison of backends from a bench CSV.

Usage: bench_compare.py results/bench_baseline.csv [baseline_backend]

Columns: backend,category,name,bytes,median_ns,p10_ns,p90_ns,iters
Prints, per (category,name,bytes), each backend's median plus a ratio vs the
chosen baseline (default: clr), and GiB/s for transfer rows.
"""
import csv
import sys
from collections import defaultdict


def fmt_ns(ns: float) -> str:
    if ns < 1_000:
        return f"{ns:.0f}ns"
    if ns < 1_000_000:
        return f"{ns/1_000:.2f}us"
    return f"{ns/1_000_000:.3f}ms"


def gibps(bytes_: int, ns: float) -> str:
    if bytes_ == 0 or ns == 0:
        return ""
    return f"{bytes_ / ns:.1f} GB/s"  # bytes/ns == GB/s (decimal)


def main():
    path = sys.argv[1]
    baseline = sys.argv[2] if len(sys.argv) > 2 else "clr"
    rows = list(csv.DictReader(open(path)))
    backends = sorted({r["backend"] for r in rows})

    # key -> backend -> median_ns
    data = defaultdict(dict)
    order = []
    for r in rows:
        key = (r["category"], r["name"], int(r["bytes"]))
        if key not in data:
            order.append(key)
        data[key][r["backend"]] = float(r["median_ns"])

    others = [b for b in backends if b != baseline]
    hdr = f"{'op':<34}{'size':>10}  {baseline:>12}"
    for b in others:
        hdr += f"  {b:>12} {'ratio':>7}"
    hdr += "   note"
    print(hdr)
    print("-" * len(hdr))

    for key in order:
        cat, name, bytes_ = key
        label = f"{cat}/{name}"
        size = f"{bytes_}" if bytes_ else "-"
        base = data[key].get(baseline)
        line = f"{label:<34}{size:>10}  "
        line += f"{fmt_ns(base):>12}" if base else f"{'-':>12}"
        for b in others:
            v = data[key].get(b)
            if v and base:
                line += f"  {fmt_ns(v):>12} {v/base:>6.2f}x"
            elif v:
                line += f"  {fmt_ns(v):>12} {'-':>7}"
            else:
                line += f"  {'-':>12} {'-':>7}"
        if cat == "xfer" and base:
            line += f"   {baseline}={gibps(bytes_, base)}"
            for b in others:
                v = data[key].get(b)
                if v:
                    line += f" {b}={gibps(bytes_, v)}"
        print(line)


if __name__ == "__main__":
    main()
