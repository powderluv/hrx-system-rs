//! Rust port of libhrx/src/libhrx/status.c — the public status ABI.
#![allow(non_snake_case)]

use core::ffi::{c_char, c_void};

use crate::common::*;

#[no_mangle]
pub extern "C" fn hrx_make_status(code: i32, message: *const c_char) -> HrxStatus {
    crate::common::hrx_make_status(code, message)
}

#[no_mangle]
pub extern "C" fn hrx_status_code(status: HrxStatus) -> i32 {
    if hrx_status_is_ok(status) {
        return HRX_STATUS_OK;
    }
    unsafe { (*status).code }
}

/// Allocates `*out_message` via libc strdup (caller frees with
/// hrx_status_free_message). Returns an HRX status (INVALID_ARGUMENT if
/// out_message is NULL).
#[no_mangle]
pub unsafe extern "C" fn hrx_status_to_string(
    status: HrxStatus,
    out_message: *mut *mut c_char,
    out_length: *mut usize,
) -> HrxStatus {
    if out_message.is_null() {
        return hrx_make_status(
            HrxStatusCode::InvalidArgument as i32,
            c"out_message is NULL".as_ptr(),
        );
    }
    if hrx_status_is_ok(status) {
        let ok = c"OK";
        *out_message = libc::strdup(ok.as_ptr());
        if !out_length.is_null() {
            *out_length = 2; // strlen("OK")
        }
        return hrx_ok_status();
    }
    let msg = (*status).message;
    let src = if msg.is_null() { c"(no message)".as_ptr() } else { msg as *const c_char };
    *out_message = libc::strdup(src);
    if !out_length.is_null() {
        *out_length = libc::strlen(src);
    }
    hrx_ok_status()
}

#[no_mangle]
pub unsafe extern "C" fn hrx_status_free_message(message: *mut c_char) {
    libc::free(message as *mut c_void);
}

#[no_mangle]
pub unsafe extern "C" fn hrx_status_ignore(status: HrxStatus) {
    if hrx_status_is_ok(status) {
        return;
    }
    libc::free((*status).message as *mut c_void);
    libc::free(status as *mut c_void);
}
