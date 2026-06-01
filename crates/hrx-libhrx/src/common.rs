//! Shared types and the IREE<->HRX status conversion, mirroring hrx_internal.h
//! and status.c.

use core::ffi::{c_char, c_void};
use iree_sys as iree;

/// `hrx_status_code_t` (hrx_runtime.h). Values match IREE's status codes by
/// convention (the C asserts this).
#[repr(i32)]
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[allow(dead_code)]
pub enum HrxStatusCode {
    Ok = 0,
    Cancelled = 1,
    Unknown = 2,
    InvalidArgument = 3,
    DeadlineExceeded = 4,
    NotFound = 5,
    AlreadyExists = 6,
    PermissionDenied = 7,
    OutOfMemory = 8, // IREE: RESOURCE_EXHAUSTED
    FailedPrecondition = 9,
    Aborted = 10,
    OutOfRange = 11,
    Unimplemented = 12,
    Internal = 13,
    Unavailable = 14,
    DataLoss = 15,
}

pub const HRX_STATUS_OK: i32 = 0;

/// `struct hrx_status_s` — heap payload for an error. `hrx_status_t` is a
/// pointer to this; NULL means OK. Field order/layout matches the C struct.
#[repr(C)]
pub struct HrxStatusS {
    pub code: i32,
    pub message: *mut c_char, // strdup'd, may be null
}

/// `hrx_status_t` — opaque pointer, NULL = OK.
pub type HrxStatus = *mut HrxStatusS;

/// `hrx_host_allocator_t` — {self, ctl}, layout-identical to iree_allocator_t.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct HrxHostAllocator {
    pub self_: *mut c_void,
    pub ctl: *mut c_void,
}

impl HrxHostAllocator {
    #[inline]
    pub fn to_iree(self) -> iree::iree_allocator_t {
        iree::iree_allocator_t { self_: self.self_, ctl: self.ctl }
    }
}

#[inline]
pub fn hrx_ok_status() -> HrxStatus {
    core::ptr::null_mut()
}

#[inline]
pub fn hrx_status_is_ok(s: HrxStatus) -> bool {
    s.is_null()
}

/// Internal equivalent of `hrx_status_ignore`: free an error status's payload
/// (used where the C code calls hrx_status_ignore on a discarded status).
#[inline]
pub fn hrx_status_drop(s: HrxStatus) {
    if s.is_null() {
        return;
    }
    unsafe {
        libc::free((*s).message as *mut core::ffi::c_void);
        libc::free(s as *mut core::ffi::c_void);
    }
}

/// Mirror of `hrx_make_status`: OK code returns NULL; otherwise malloc a payload
/// and strdup the message. Uses libc malloc/strdup so `hrx_status_ignore`
/// (libc free) and the C reference free the same way.
pub fn hrx_make_status(code: i32, message: *const c_char) -> HrxStatus {
    if code == HRX_STATUS_OK {
        return hrx_ok_status();
    }
    unsafe {
        let s = libc::malloc(core::mem::size_of::<HrxStatusS>()) as *mut HrxStatusS;
        if s.is_null() {
            return core::ptr::null_mut(); // OOM making error.
        }
        (*s).code = code;
        (*s).message = if message.is_null() {
            core::ptr::null_mut()
        } else {
            libc::strdup(message)
        };
        s
    }
}

/// Map an IREE status code to the HRX code (status.c switch). The two enums
/// mostly coincide; the exceptions are RESOURCE_EXHAUSTED->OUT_OF_MEMORY and the
/// default->INTERNAL.
fn hrx_code_from_iree(iree_code: i32) -> i32 {
    use HrxStatusCode::*;
    // IREE codes (status.h): INVALID_ARGUMENT=3, NOT_FOUND=5, ALREADY_EXISTS=6,
    // OUT_OF_RANGE=11, UNIMPLEMENTED=12, UNAVAILABLE=14, RESOURCE_EXHAUSTED=8,
    // DEADLINE_EXCEEDED=4.
    match iree_code {
        3 => InvalidArgument as i32,
        5 => NotFound as i32,
        6 => AlreadyExists as i32,
        11 => OutOfRange as i32,
        12 => Unimplemented as i32,
        14 => Unavailable as i32,
        8 => OutOfMemory as i32,
        4 => DeadlineExceeded as i32,
        _ => Internal as i32,
    }
}

/// Mirror of `hrx_status_from_iree`: converts + consumes an IREE status,
/// extracting its message. Returns an HRX status.
pub fn hrx_status_from_iree(iree_status: iree::iree_status_t) -> HrxStatus {
    if iree::status_is_ok(iree_status) {
        return hrx_ok_status();
    }
    let hrx_code = hrx_code_from_iree(iree::status_code(iree_status));
    unsafe {
        let allocator = iree::allocator_system();
        let mut msg: *mut c_char = core::ptr::null_mut();
        let mut msg_len: usize = 0;
        let ok = iree::iree_status_to_string(iree_status, &allocator, &mut msg, &mut msg_len);
        if !ok {
            iree::iree_status_free(iree_status);
            let m = c"IREE error (could not format message)";
            return hrx_make_status(hrx_code, m.as_ptr());
        }
        let result = hrx_make_status(hrx_code, msg);
        iree::iree_allocator_free(allocator, msg as *mut c_void);
        iree::iree_status_free(iree_status);
        result
    }
}
