#!/usr/bin/env bash
# Unsafe-block ratchet (plan C5): fail if any tracked crate's count of `unsafe {`
# operation blocks rises above its recorded baseline, preventing unsafe creep as
# the port evolves. We count `unsafe {` blocks (the actual unsafe operations), not
# `pub unsafe extern "C" fn` ABI declarations (those are an unavoidable, fixed
# surface). Baselines live in scripts/unsafe_baseline.txt.
#
# An increase FAILS (justify the new unsafe, or re-baseline in the same commit). A
# decrease is advisory — lower the baseline to lock in the improvement. The grep is
# source-text based, so it also counts `#[cfg(test)]`/`cfg(miri)` blocks; that is
# intentional — every unsafe addition, test or not, surfaces as a baseline bump in
# review.
set -euo pipefail
cd "$(dirname "$0")/.."

fail=0
while read -r crate baseline; do
  case "$crate" in ''|\#*) continue ;; esac
  cur=$(grep -rho 'unsafe {' "crates/$crate/src/" | wc -l | tr -d ' ')
  if [ "$cur" -gt "$baseline" ]; then
    echo "FAIL: $crate has $cur unsafe blocks > baseline $baseline (+$((cur - baseline))). Justify the new unsafe or refactor it away; if intended, bump scripts/unsafe_baseline.txt in this commit."
    fail=1
  elif [ "$cur" -lt "$baseline" ]; then
    echo "RATCHET-DOWN: $crate dropped to $cur unsafe blocks (baseline $baseline) — lower the baseline in scripts/unsafe_baseline.txt to lock it in."
  else
    echo "OK: $crate has $cur unsafe blocks (== baseline)."
  fi
done < scripts/unsafe_baseline.txt

exit $fail
