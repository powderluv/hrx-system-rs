//! Compile-time ABI layout assertions for the by-value IREE structs modeled in
//! this crate. They guard against silent drift between our `#[repr(C)]` models
//! and the real IREE headers — the class of bug the `iree_hal_buffer_params_t`
//! `min_alignment` omission once was (a 32-byte struct modeled as 24 bytes,
//! read 8 bytes out of bounds via the by-pointer ABI). Values were probed with
//! clang against the IREE headers; `scripts/check_abi_layout.sh` re-probes the
//! headers and compares, catching drift on the C side too.
use crate::fem::iree_hal_external_buffer_t;
use crate::init::{
    iree_hal_buffer_params_t, iree_hal_buffer_ref_list_t, iree_hal_buffer_ref_t,
    iree_hal_dispatch_config_t, iree_hal_semaphore_list_t,
};
use crate::iree_const_byte_span_t;
use core::mem::{align_of, offset_of, size_of};

const _: () = {
    // iree_hal_buffer_params_t (32B): usage u32@0, access u16@4, type u32@8,
    // queue_affinity u64@16, min_alignment u64@24.
    assert!(size_of::<iree_hal_buffer_params_t>() == 32);
    assert!(offset_of!(iree_hal_buffer_params_t, usage) == 0);
    assert!(offset_of!(iree_hal_buffer_params_t, access) == 4);
    assert!(offset_of!(iree_hal_buffer_params_t, type_) == 8);
    assert!(offset_of!(iree_hal_buffer_params_t, queue_affinity) == 16);
    assert!(offset_of!(iree_hal_buffer_params_t, min_alignment) == 24);

    // iree_hal_dispatch_config_t (64B, align 8).
    assert!(size_of::<iree_hal_dispatch_config_t>() == 64);
    assert!(align_of::<iree_hal_dispatch_config_t>() == 8);
    assert!(offset_of!(iree_hal_dispatch_config_t, workgroup_size) == 0);
    assert!(offset_of!(iree_hal_dispatch_config_t, workgroup_count) == 12);
    assert!(offset_of!(iree_hal_dispatch_config_t, workgroup_count_ref) == 24);
    assert!(offset_of!(iree_hal_dispatch_config_t, dynamic_workgroup_local_memory) == 56);

    // iree_hal_buffer_ref_t (32B): buffer*@8, offset@16, length@24.
    assert!(size_of::<iree_hal_buffer_ref_t>() == 32);
    assert!(offset_of!(iree_hal_buffer_ref_t, buffer) == 8);
    assert!(offset_of!(iree_hal_buffer_ref_t, offset) == 16);
    assert!(offset_of!(iree_hal_buffer_ref_t, length) == 24);

    // iree_hal_buffer_ref_list_t (16B): count@0, values@8.
    assert!(size_of::<iree_hal_buffer_ref_list_t>() == 16);
    assert!(offset_of!(iree_hal_buffer_ref_list_t, count) == 0);
    assert!(offset_of!(iree_hal_buffer_ref_list_t, values) == 8);

    // iree_hal_semaphore_list_t (24B): count@0, semaphores@8, payload_values@16.
    assert!(size_of::<iree_hal_semaphore_list_t>() == 24);
    assert!(offset_of!(iree_hal_semaphore_list_t, count) == 0);
    assert!(offset_of!(iree_hal_semaphore_list_t, semaphores) == 8);
    assert!(offset_of!(iree_hal_semaphore_list_t, payload_values) == 16);

    // iree_hal_external_buffer_t (24B): type u32@0, flags u32@4, size u64@8, handle ptr@16.
    assert!(size_of::<iree_hal_external_buffer_t>() == 24);
    assert!(offset_of!(iree_hal_external_buffer_t, size) == 8);
    assert!(offset_of!(iree_hal_external_buffer_t, handle_ptr) == 16);

    // iree_const_byte_span_t (16B): data*@0, data_length@8.
    assert!(size_of::<iree_const_byte_span_t>() == 16);
    assert!(offset_of!(iree_const_byte_span_t, data) == 0);
    assert!(offset_of!(iree_const_byte_span_t, data_length) == 8);
};
