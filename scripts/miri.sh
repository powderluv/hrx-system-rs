#!/usr/bin/env bash
# Miri over the pure-Rust ownership machinery of the port — the Arc handle
# boundary today (handle.rs), expanding to the full object model as the in-memory
# mock IREE backend lets more of it run (see crates/iree-sys `mock` feature).
#
# Miri interprets MIR and never links or calls the real IREE C archives, so this
# lane is GPU-free and needs no prebuilt build — the `#[ctor]` load constructor is
# dropped under cfg(miri) and the FFI-touching paths are exercised against the
# mock backend, not libIREE. Miri's default checks (use-after-free, double-free,
# invalid aliasing, uninitialised reads) plus its leak check run on every test.
set -euo pipefail

TOOLCHAIN="${MIRI_TOOLCHAIN:-nightly-2026-04-03}"
cd "$(dirname "$0")/.."

# Pure-Rust handle boundary (no FFI, no mock needed).
cargo "+$TOOLCHAIN" miri test -p hrx-libhrx --lib handle::tests "$@"
