//! Low-level FFI to the IREE C runtime + HRX streaming layer.
//!
//! Phase 2 foundation: this crate links the static IREE archives and exposes
//! the raw `iree_*` symbols the Rust libhrx reimplementation will call. For now
//! it declares just enough to prove static linkage + execution from Rust
//! (see `tests/`); the full surface is filled in incrementally (bindgen over
//! the IREE headers is the intended path).
#![allow(non_camel_case_types)]

use core::ffi::c_void;

/// `iree_allocator_t` — a {self, ctl-fn} pair (2 words). Layout-stable C ABI.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct iree_allocator_t {
    pub self_: *mut c_void,
    pub ctl: *mut c_void,
}

extern "C" {
    /// The libc-backed allocator control fn (real exported symbol). Build a
    /// system allocator as `{ null, iree_allocator_libc_ctl }` — this mirrors
    /// the `iree_allocator_system()` static-inline in the C headers.
    pub fn iree_allocator_libc_ctl(
        self_: *mut c_void,
        command: i32,
        params: *const c_void,
        inout_ptr: *mut *mut c_void,
    ) -> i32;

    pub fn iree_allocator_malloc(
        allocator: iree_allocator_t,
        byte_length: u64,
        out_ptr: *mut *mut c_void,
    ) -> i32;

    pub fn iree_allocator_free(allocator: iree_allocator_t, ptr: *mut c_void);
}

/// Construct the system (libc) allocator, equivalent to the C
/// `iree_allocator_system()` static-inline.
pub fn allocator_system() -> iree_allocator_t {
    iree_allocator_t {
        self_: core::ptr::null_mut(),
        ctl: iree_allocator_libc_ctl as *mut c_void,
    }
}
