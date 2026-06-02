#!/usr/bin/env bash
# ABI gate (GPU-free): re-probe the real IREE headers and assert the by-value
# struct layouts our #[repr(C)] models depend on still hold. The Rust side has
# matching compile-time asserts in crates/iree-sys/src/abi_layout.rs; this script
# guards the C/header side, so drift on either side fails a build.
#
#   HRX_SYSTEM_SRC=~/github/hrx-system bash scripts/check_abi_layout.sh
set -u
SRC="${HRX_SYSTEM_SRC:-$HOME/github/hrx-system}"
INC="$SRC/runtime/src"
[ -d "$INC/iree/hal" ] || { echo "FAIL: IREE headers not found under $INC (set HRX_SYSTEM_SRC)"; exit 2; }

CC="${CC:-cc}"
probe="$(mktemp --suffix=.c)"
trap 'rm -f "$probe" "${probe%.c}"' EXIT

cat > "$probe" <<'EOF'
#include <stddef.h>
#include "iree/hal/buffer.h"
#include "iree/hal/allocator.h"
#include "iree/hal/command_buffer.h"
#include "iree/hal/semaphore.h"
#include "iree/base/allocator.h"

#define A(c) _Static_assert((c), #c)

/* iree_hal_buffer_params_t (32B; min_alignment @24 — the bug class) */
A(sizeof(iree_hal_buffer_params_t) == 32);
A(offsetof(iree_hal_buffer_params_t, usage) == 0);
A(offsetof(iree_hal_buffer_params_t, type) == 8);
A(offsetof(iree_hal_buffer_params_t, queue_affinity) == 16);
A(offsetof(iree_hal_buffer_params_t, min_alignment) == 24);

/* iree_hal_dispatch_config_t (64B) */
A(sizeof(iree_hal_dispatch_config_t) == 64);
A(offsetof(iree_hal_dispatch_config_t, workgroup_size) == 0);
A(offsetof(iree_hal_dispatch_config_t, workgroup_count) == 12);
A(offsetof(iree_hal_dispatch_config_t, workgroup_count_ref) == 24);
A(offsetof(iree_hal_dispatch_config_t, dynamic_workgroup_local_memory) == 56);

/* iree_hal_buffer_ref_t (32B) + list (16B) */
A(sizeof(iree_hal_buffer_ref_t) == 32);
A(offsetof(iree_hal_buffer_ref_t, buffer) == 8);
A(offsetof(iree_hal_buffer_ref_t, offset) == 16);
A(offsetof(iree_hal_buffer_ref_t, length) == 24);
A(sizeof(iree_hal_buffer_ref_list_t) == 16);

/* iree_hal_semaphore_list_t (24B) */
A(sizeof(iree_hal_semaphore_list_t) == 24);
A(offsetof(iree_hal_semaphore_list_t, count) == 0);
A(offsetof(iree_hal_semaphore_list_t, semaphores) == 8);
A(offsetof(iree_hal_semaphore_list_t, payload_values) == 16);

/* iree_hal_external_buffer_t (24B) */
A(sizeof(iree_hal_external_buffer_t) == 24);
A(offsetof(iree_hal_external_buffer_t, size) == 8);

/* iree_const_byte_span_t (16B) */
A(sizeof(iree_const_byte_span_t) == 16);

int main(void) { return 0; }
EOF

if "$CC" -I "$INC" -c "$probe" -o "${probe%.c}.o" 2>/tmp/abi_probe.err; then
  echo "PASS: IREE header layouts match the Rust #[repr(C)] models"
  rm -f "${probe%.c}.o"
else
  echo "FAIL: IREE header layout drift — a #[repr(C)] model is out of date:"
  cat /tmp/abi_probe.err
  exit 1
fi
