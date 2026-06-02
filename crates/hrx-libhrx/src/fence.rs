//! Rust port of libhrx/src/libhrx/fence.c — timeline fence.
//!
//! Phase-1 owned-object model: the opaque `hrx_fence_t` is the `Arc` data pointer
//! of an `HrxFenceS`; `hrx_fence_retain`/`release` are `Arc` refcount operations
//! (the same atomics the old hand-rolled refcount used), and the underlying IREE
//! fence is released exactly once by `HalFence`'s `Drop` on the last release —
//! no `libc::calloc`/`free`, no manual `iree_hal_fence_release` accounting.
#![allow(non_snake_case)]

use crate::common::*;
use crate::handle::{handle_ref, handle_release, handle_retain, into_handle};
use crate::semaphore::HrxSemaphore;
use iree_hal::{fence_create, fence_create_at, HalFence};
use iree_sys::fem;
use iree_sys::init as ireei;

/// Internal object behind the opaque `hrx_fence_t`. Owns one IREE fence
/// reference via `HalFence`; the handle's reference count is the `Arc` strong
/// count.
pub struct HrxFenceS {
    hal: HalFence,
}
pub type HrxFence = *mut HrxFenceS;

/// Borrow the raw IREE fence pointer behind a handle (for value_list's vm-ref
/// adapter, which needs the `iree_hal_fence_t*`).
///
/// # Safety
/// `fence` must be a live `hrx_fence_t`.
pub(crate) unsafe fn fence_hal_ptr(fence: HrxFence) -> *mut fem::iree_hal_fence_t {
    handle_ref(fence).hal.as_ptr()
}

#[no_mangle]
pub unsafe extern "C" fn hrx_fence_create(capacity: usize, fence: *mut HrxFence) -> HrxStatus {
    if fence.is_null() {
        return hrx_make_status(HrxStatusCode::InvalidArgument as i32, c"fence is NULL".as_ptr());
    }
    *fence = core::ptr::null_mut();
    match fence_create(capacity) {
        Ok(hal) => {
            *fence = into_handle(HrxFenceS { hal });
            hrx_ok_status()
        }
        Err(s) => hrx_status_from_iree(s),
    }
}

#[no_mangle]
pub unsafe extern "C" fn hrx_fence_create_at(
    semaphore: HrxSemaphore,
    value: u64,
    fence: *mut HrxFence,
) -> HrxStatus {
    if semaphore.is_null() || fence.is_null() {
        return hrx_make_status(
            HrxStatusCode::InvalidArgument as i32,
            c"semaphore or fence is NULL".as_ptr(),
        );
    }
    *fence = core::ptr::null_mut();
    match fence_create_at(crate::semaphore::semaphore_hal_ptr(semaphore), value) {
        Ok(hal) => {
            *fence = into_handle(HrxFenceS { hal });
            hrx_ok_status()
        }
        Err(s) => hrx_status_from_iree(s),
    }
}

#[no_mangle]
pub unsafe extern "C" fn hrx_fence_retain(fence: HrxFence) {
    handle_retain(fence);
}

#[no_mangle]
pub unsafe extern "C" fn hrx_fence_release(fence: HrxFence) {
    handle_release(fence);
}

#[no_mangle]
pub unsafe extern "C" fn hrx_fence_insert(
    fence: HrxFence,
    semaphore: HrxSemaphore,
    value: u64,
) -> HrxStatus {
    if fence.is_null() || semaphore.is_null() {
        return hrx_make_status(
            HrxStatusCode::InvalidArgument as i32,
            c"fence or semaphore is NULL".as_ptr(),
        );
    }
    hrx_status_from_iree(handle_ref(fence).hal.insert(crate::semaphore::semaphore_hal_ptr(semaphore), value))
}

#[no_mangle]
pub unsafe extern "C" fn hrx_fence_extend(into_fence: HrxFence, from_fence: HrxFence) -> HrxStatus {
    if into_fence.is_null() || from_fence.is_null() {
        return hrx_make_status(
            HrxStatusCode::InvalidArgument as i32,
            c"into_fence or from_fence is NULL".as_ptr(),
        );
    }
    let into_ref = handle_ref(into_fence);
    let from_ref = handle_ref(from_fence);
    hrx_status_from_iree(into_ref.hal.extend(&from_ref.hal))
}

#[no_mangle]
pub unsafe extern "C" fn hrx_fence_signal(fence: HrxFence) -> HrxStatus {
    if fence.is_null() {
        return hrx_make_status(HrxStatusCode::InvalidArgument as i32, c"fence is NULL".as_ptr());
    }
    hrx_status_from_iree(handle_ref(fence).hal.signal())
}

#[no_mangle]
pub unsafe extern "C" fn hrx_fence_wait(fence: HrxFence, timeout_ns: u64) -> HrxStatus {
    if fence.is_null() {
        return hrx_make_status(HrxStatusCode::InvalidArgument as i32, c"fence is NULL".as_ptr());
    }
    let timeout = if timeout_ns == u64::MAX {
        ireei::iree_timeout_t::infinite()
    } else if timeout_ns == 0 {
        ireei::iree_timeout_t::immediate()
    } else {
        ireei::iree_timeout_t::relative_ns(timeout_ns as i64)
    };
    hrx_status_from_iree(handle_ref(fence).hal.wait(timeout))
}
