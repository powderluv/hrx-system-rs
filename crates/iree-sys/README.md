# iree-sys

Low-level Rust FFI to the IREE C runtime + HRX streaming layer, linked from the
static archives produced by the hrx-system CMake build.

## Prerequisite: build the C reference

```bash
ROCM_DEVEL=$HOME/github/therock-nightly-venv/lib/python3.12/site-packages/_rocm_sdk_devel
cmake -B $HOME/github/hrx-system-build -S $HOME/github/hrx-system -GNinja \
  -DCMAKE_BUILD_TYPE=Release \
  -DCMAKE_C_COMPILER=$ROCM_DEVEL/lib/llvm/bin/clang \
  -DCMAKE_CXX_COMPILER=$ROCM_DEVEL/lib/llvm/bin/clang++ \
  -DCMAKE_PREFIX_PATH=$ROCM_DEVEL \
  -DIREE_BUILD_TESTS=OFF -DLIBHRX_BUILD_CTS=OFF -DHRX_INSTALL_TESTS=OFF \
  -DIREE_ENABLE_WERROR_FLAG=OFF
ninja -C $HOME/github/hrx-system-build
```

`build.rs` reads `iree_archives.txt` (108 archives in link order, captured from
the libhrx.so link line) and re-roots them onto `$HRX_BUILD_DIR`
(default `~/github/hrx-system-build`), wrapped in --start-group/--end-group.

## Status

Foundation only: declares enough to prove static linkage + execution from Rust
(`cargo test -p iree-sys` allocates/frees via the IREE system allocator). The
full `iree_*` / `iree_hal_streaming_*` surface is filled in incrementally.
