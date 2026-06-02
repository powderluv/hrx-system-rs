//! Low-level FFI to the IREE C runtime + HRX streaming layer.
//!
//! Phase 2: this crate links the static IREE archives and exposes the raw
//! `iree_*` symbols the Rust libhrx reimplementation calls. Surface is filled in
//! incrementally; this batch covers what the GPU-independent libhrx modules
//! (status, host_allocator, value_list) need.
//!
//! `iree_status_t` is an opaque pointer whose low 5 bits carry the status code
//! when there is no heap payload (`iree_status_from_code`); a non-trivial error
//! is a heap pointer. `iree_status_code(p) == ((uintptr_t)p) & 0x1F`, and
//! `iree_status_is_ok(p) == (p == NULL)`. These are macros in C; reproduced
//! here as `status_code()` / `status_is_ok()`.
#![allow(non_camel_case_types)]

use core::ffi::{c_char, c_void};

pub type iree_host_size_t = usize;
pub type iree_status_t = *mut c_void;
pub type iree_status_code_t = i32;
pub type iree_vm_list_t = c_void;

pub const IREE_STATUS_CODE_MASK: usize = 0x1F;
pub const IREE_VM_VALUE_TYPE_I64: i32 = 4;
pub const IREE_VM_TYPE_CAPACITY_DEFAULT: iree_host_size_t = 32;
pub const IREE_ASYNC_WAIT_MODE_ALL: i32 = 0;

// Opaque IREE handle types (always behind pointers).
pub type iree_vm_instance_t = c_void;
pub type iree_async_proactor_pool_t = c_void;
pub type iree_async_frontier_tracker_t = c_void;
pub type iree_task_executor_t = c_void;
pub type iree_hal_executable_loader_t = c_void;
pub type iree_hal_allocator_t = c_void;
pub type iree_hal_driver_t = c_void;
pub type iree_hal_device_t = c_void;
pub type iree_hal_device_group_t = c_void;
pub type iree_async_proactor_t = c_void;
pub type iree_async_notification_t = c_void;
pub type iree_hal_pool_t = c_void;

pub mod fem;
pub mod init;
mod abi_layout;

/// `iree_allocator_null()` (inline) — a zeroed allocator (no self, no ctl).
#[inline]
pub fn allocator_null() -> iree_allocator_t {
    iree_allocator_t { self_: core::ptr::null_mut(), ctl: core::ptr::null_mut() }
}

/// `iree_allocator_t` — a {self, ctl-fn} pair (2 words). Layout-stable C ABI.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct iree_allocator_t {
    pub self_: *mut c_void,
    pub ctl: *mut c_void,
}

/// `iree_const_byte_span_t` — {data, length}.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct iree_const_byte_span_t {
    pub data: *const u8,
    pub data_length: iree_host_size_t,
}

/// `iree_vm_value_t` = `{ i32 type; union { ...; i64; u8[8] } }`. The union is
/// 8-byte aligned, so the struct is 16 bytes: `type_` at offset 0, 4 bytes
/// padding, then the 8-byte payload at offset 8. A naive `{i32, [u8;8]}` would
/// be 12 bytes and place the i64 at offset 4 — wrong. The explicit `_pad` +
/// `align(8)` reproduces the real layout.
#[repr(C, align(8))]
#[derive(Clone, Copy)]
pub struct iree_vm_value_t {
    pub type_: i32,
    pub _pad: i32,
    pub storage: [u8; 8],
}

impl iree_vm_value_t {
    pub fn make_i64(v: i64) -> Self {
        iree_vm_value_t {
            type_: IREE_VM_VALUE_TYPE_I64,
            _pad: 0,
            storage: v.to_ne_bytes(),
        }
    }
    pub fn as_i64(&self) -> i64 {
        i64::from_ne_bytes(self.storage)
    }
}

/// `iree_vm_type_def_t` is a single machine word; the undefined type def is all
/// zero bits.
pub type iree_vm_type_def_t = usize;
pub const IREE_VM_TYPE_DEF_UNDEFINED: iree_vm_type_def_t = 0;

/// `iree_vm_ref_t` — {ptr, type}. The null ref is all-zero.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct iree_vm_ref_t {
    pub ptr: *mut c_void,
    pub type_: usize,
}

impl iree_vm_ref_t {
    #[inline]
    pub fn null() -> Self {
        iree_vm_ref_t { ptr: core::ptr::null_mut(), type_: 0 }
    }
}

extern "C" {
    // --- host allocator ---
    pub fn iree_allocator_libc_ctl(
        self_: *mut c_void,
        command: i32,
        params: *const c_void,
        inout_ptr: *mut *mut c_void,
    ) -> iree_status_t;
    pub fn iree_allocator_malloc(
        allocator: iree_allocator_t,
        byte_length: iree_host_size_t,
        out_ptr: *mut *mut c_void,
    ) -> iree_status_t;
    pub fn iree_allocator_malloc_uninitialized(
        allocator: iree_allocator_t,
        byte_length: iree_host_size_t,
        out_ptr: *mut *mut c_void,
    ) -> iree_status_t;
    pub fn iree_allocator_realloc(
        allocator: iree_allocator_t,
        byte_length: iree_host_size_t,
        inout_ptr: *mut *mut c_void,
    ) -> iree_status_t;
    pub fn iree_allocator_clone(
        allocator: iree_allocator_t,
        source_bytes: iree_const_byte_span_t,
        out_ptr: *mut *mut c_void,
    ) -> iree_status_t;
    pub fn iree_allocator_malloc_aligned(
        allocator: iree_allocator_t,
        byte_length: iree_host_size_t,
        min_alignment: iree_host_size_t,
        offset: iree_host_size_t,
        out_ptr: *mut *mut c_void,
    ) -> iree_status_t;
    pub fn iree_allocator_realloc_aligned(
        allocator: iree_allocator_t,
        byte_length: iree_host_size_t,
        min_alignment: iree_host_size_t,
        offset: iree_host_size_t,
        inout_ptr: *mut *mut c_void,
    ) -> iree_status_t;
    pub fn iree_allocator_free(allocator: iree_allocator_t, ptr: *mut c_void);
    pub fn iree_allocator_free_aligned(allocator: iree_allocator_t, ptr: *mut c_void);

    // --- status ---
    pub fn iree_status_free(status: iree_status_t);
    pub fn iree_status_to_string(
        status: iree_status_t,
        allocator: *const iree_allocator_t,
        out_buffer: *mut *mut c_char,
        out_buffer_length: *mut iree_host_size_t,
    ) -> bool;
    /// Backs `iree_make_status(code, fmt, ...)` (a macro). Variadic printf-style.
    pub fn iree_status_allocate_f(
        code: iree_status_code_t,
        file: *const c_char,
        line: u32,
        format: *const c_char,
        ...
    ) -> iree_status_t;

    // --- vm list ---
    pub fn iree_vm_list_create(
        element_type: iree_vm_type_def_t,
        initial_capacity: iree_host_size_t,
        allocator: iree_allocator_t,
        out_list: *mut *mut iree_vm_list_t,
    ) -> iree_status_t;
    pub fn iree_vm_list_retain(list: *mut iree_vm_list_t);
    pub fn iree_vm_list_release(list: *mut iree_vm_list_t);
    pub fn iree_vm_list_size(list: *const iree_vm_list_t) -> iree_host_size_t;
    pub fn iree_vm_list_push_value(
        list: *mut iree_vm_list_t,
        value: *const iree_vm_value_t,
    ) -> iree_status_t;
    pub fn iree_vm_list_get_value_as(
        list: *const iree_vm_list_t,
        i: iree_host_size_t,
        value_type: i32,
        out_value: *mut iree_vm_value_t,
    ) -> iree_status_t;
    pub fn iree_vm_list_push_ref_move(
        list: *mut iree_vm_list_t,
        value: *mut iree_vm_ref_t,
    ) -> iree_status_t;
}

/// Construct the system (libc) allocator, equivalent to the C
/// `iree_allocator_system()` static-inline.
#[inline]
pub fn allocator_system() -> iree_allocator_t {
    iree_allocator_t {
        self_: core::ptr::null_mut(),
        ctl: iree_allocator_libc_ctl as *mut c_void,
    }
}

/// `iree_status_code(status)` — low 5 bits of the pointer.
#[inline]
pub fn status_code(status: iree_status_t) -> iree_status_code_t {
    (status as usize & IREE_STATUS_CODE_MASK) as iree_status_code_t
}

/// `iree_status_is_ok(status)` — NULL is OK.
#[inline]
pub fn status_is_ok(status: iree_status_t) -> bool {
    status.is_null()
}

/// `iree_status_from_code(code)` (inline) — a code-only status with no heap
/// payload (the code is encoded inline in the pointer; freeing it is a no-op).
#[inline]
pub fn iree_status_from_code(code: iree_status_code_t) -> iree_status_t {
    (code as usize & IREE_STATUS_CODE_MASK) as iree_status_t
}

/// IREE status codes used by hand-rolled error returns.
pub const IREE_STATUS_INVALID_ARGUMENT: iree_status_code_t = 3;
pub const IREE_STATUS_FAILED_PRECONDITION: iree_status_code_t = 9;
