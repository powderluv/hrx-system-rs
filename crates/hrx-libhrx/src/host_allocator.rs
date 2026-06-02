//! Rust port of libhrx/src/libhrx/host_allocator.c — the public host-allocator
//! ABI over iree_allocator_*.
#![allow(non_snake_case)]

use core::ffi::c_void;
use core::sync::atomic::{AtomicPtr, Ordering};

use crate::common::*;
use iree_sys as iree;

/// Exported global `hrx_host_allocator_system_value` (HRX_API extern). The C
/// version is a plain global filled by a __attribute__((constructor)). We must
/// export the same symbol with the same {self, ctl} layout and initialize it at
/// load. The fields are filled by the ctor below; we store them in atomics that
/// alias the exported struct's two words.
///
/// To export a C-ABI global of exactly `HrxHostAllocator` layout, we use a
/// static with #[no_mangle]. It's filled at load by `host_allocator_init`.
#[no_mangle]
pub static mut hrx_host_allocator_system_value: HrxHostAllocator = HrxHostAllocator {
    self_: core::ptr::null_mut(),
    ctl: core::ptr::null_mut(),
};

// Keep an atomic mirror so init ordering is well-defined even though the
// exported symbol is a plain static (the ctor runs before any HRX call).
static CTL: AtomicPtr<c_void> = AtomicPtr::new(core::ptr::null_mut());

#[ctor::ctor]
fn host_allocator_init() {
    let sys = iree::allocator_system();
    CTL.store(sys.ctl, Ordering::SeqCst);
    // Safe: runs single-threaded at load before any HRX entry point.
    unsafe {
        let p = core::ptr::addr_of_mut!(hrx_host_allocator_system_value);
        (*p).self_ = sys.self_;
        (*p).ctl = sys.ctl;
    }
}

/// # Safety
/// Pointer arguments (`out_ptr`/`inout_ptr`/`src`/`ptr`), where present, must be
/// valid for the documented access and sizes; `allocator` must be a real
/// `hrx_host_allocator_t`.
#[no_mangle]
pub unsafe extern "C" fn hrx_host_allocator_malloc(
    allocator: HrxHostAllocator,
    byte_length: usize,
    out_ptr: *mut *mut c_void,
) -> HrxStatus {
    hrx_status_from_iree(unsafe {
        iree::iree_allocator_malloc(allocator.to_iree(), byte_length, out_ptr)
    })
}

/// # Safety
/// Pointer arguments (`out_ptr`/`inout_ptr`/`src`/`ptr`), where present, must be
/// valid for the documented access and sizes; `allocator` must be a real
/// `hrx_host_allocator_t`.
#[no_mangle]
pub unsafe extern "C" fn hrx_host_allocator_malloc_uninitialized(
    allocator: HrxHostAllocator,
    byte_length: usize,
    out_ptr: *mut *mut c_void,
) -> HrxStatus {
    hrx_status_from_iree(unsafe {
        iree::iree_allocator_malloc_uninitialized(allocator.to_iree(), byte_length, out_ptr)
    })
}

/// # Safety
/// Pointer arguments (`out_ptr`/`inout_ptr`/`src`/`ptr`), where present, must be
/// valid for the documented access and sizes; `allocator` must be a real
/// `hrx_host_allocator_t`.
#[no_mangle]
pub unsafe extern "C" fn hrx_host_allocator_realloc(
    allocator: HrxHostAllocator,
    byte_length: usize,
    inout_ptr: *mut *mut c_void,
) -> HrxStatus {
    hrx_status_from_iree(unsafe {
        iree::iree_allocator_realloc(allocator.to_iree(), byte_length, inout_ptr)
    })
}

/// # Safety
/// Pointer arguments (`out_ptr`/`inout_ptr`/`src`/`ptr`), where present, must be
/// valid for the documented access and sizes; `allocator` must be a real
/// `hrx_host_allocator_t`.
#[no_mangle]
pub unsafe extern "C" fn hrx_host_allocator_clone(
    allocator: HrxHostAllocator,
    src: *const c_void,
    byte_length: usize,
    out_ptr: *mut *mut c_void,
) -> HrxStatus {
    let span = iree::iree_const_byte_span_t {
        data: src as *const u8,
        data_length: byte_length,
    };
    hrx_status_from_iree(unsafe {
        iree::iree_allocator_clone(allocator.to_iree(), span, out_ptr)
    })
}

/// # Safety
/// Pointer arguments (`out_ptr`/`inout_ptr`/`src`/`ptr`), where present, must be
/// valid for the documented access and sizes; `allocator` must be a real
/// `hrx_host_allocator_t`.
#[no_mangle]
pub unsafe extern "C" fn hrx_host_allocator_free(allocator: HrxHostAllocator, ptr: *mut c_void) {
    unsafe { iree::iree_allocator_free(allocator.to_iree(), ptr) }
}

/// # Safety
/// Pointer arguments (`out_ptr`/`inout_ptr`/`src`/`ptr`), where present, must be
/// valid for the documented access and sizes; `allocator` must be a real
/// `hrx_host_allocator_t`.
#[no_mangle]
pub unsafe extern "C" fn hrx_host_allocator_malloc_aligned(
    allocator: HrxHostAllocator,
    byte_length: usize,
    min_alignment: usize,
    offset: usize,
    out_ptr: *mut *mut c_void,
) -> HrxStatus {
    hrx_status_from_iree(unsafe {
        iree::iree_allocator_malloc_aligned(
            allocator.to_iree(), byte_length, min_alignment, offset, out_ptr,
        )
    })
}

/// # Safety
/// Pointer arguments (`out_ptr`/`inout_ptr`/`src`/`ptr`), where present, must be
/// valid for the documented access and sizes; `allocator` must be a real
/// `hrx_host_allocator_t`.
#[no_mangle]
pub unsafe extern "C" fn hrx_host_allocator_realloc_aligned(
    allocator: HrxHostAllocator,
    byte_length: usize,
    min_alignment: usize,
    offset: usize,
    inout_ptr: *mut *mut c_void,
) -> HrxStatus {
    hrx_status_from_iree(unsafe {
        iree::iree_allocator_realloc_aligned(
            allocator.to_iree(), byte_length, min_alignment, offset, inout_ptr,
        )
    })
}

/// # Safety
/// Pointer arguments (`out_ptr`/`inout_ptr`/`src`/`ptr`), where present, must be
/// valid for the documented access and sizes; `allocator` must be a real
/// `hrx_host_allocator_t`.
#[no_mangle]
pub unsafe extern "C" fn hrx_host_allocator_free_aligned(allocator: HrxHostAllocator, ptr: *mut c_void) {
    unsafe { iree::iree_allocator_free_aligned(allocator.to_iree(), ptr) }
}
