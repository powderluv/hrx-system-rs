//! Rust port of libhrx/src/libhrx/semaphore.c — timeline semaphore.
//!
//! Phase-1 owned-object model: the opaque `hrx_semaphore_t` is the `Arc` data
//! pointer of an `HrxSemaphoreS`; retain/release are `Arc` refcount ops, and on
//! the last release the fields drop in declaration order — `HalSemaphore` first
//! (releases the IREE semaphore), then `DeviceRef` (releases the device) — which
//! matches the C release order (semaphore then device). The C code's per-retain
//! device retain/release pairs cancel; here the device is held by exactly one
//! reference for the semaphore's lifetime, which is observably equivalent.
#![allow(non_snake_case)]

use crate::common::*;
use crate::device::{DeviceRef, HrxDevice};
use crate::handle::{handle_ref, handle_release, handle_retain, into_handle};
use iree_hal::{semaphore_create, HalSemaphore};
use iree_sys::init as ireei;

/// Internal object behind the opaque `hrx_semaphore_t`. Field order is
/// load-bearing for `Drop`: `hal` (IREE semaphore) releases before `device`.
pub struct HrxSemaphoreS {
    hal: HalSemaphore,
    /// RAII drop-guard: holds one device reference for the semaphore's lifetime
    /// and releases it on drop (after `hal`). Never read directly.
    #[allow(dead_code)]
    device: DeviceRef,
}
pub type HrxSemaphore = *mut HrxSemaphoreS;

/// Borrow the raw IREE semaphore pointer behind a handle (for stream/queue/fence
/// submission, which build IREE semaphore lists).
///
/// # Safety
/// `sem` must be a live `hrx_semaphore_t`.
pub(crate) unsafe fn semaphore_hal_ptr(sem: HrxSemaphore) -> *mut ireei::iree_hal_semaphore_t {
    handle_ref(sem).hal.as_ptr()
}

/// RAII guard for a semaphore handle the owner created (born with one ref) and
/// will release on drop. Used by the stream, which owns its timeline semaphore.
pub(crate) struct SemaphoreGuard(HrxSemaphore);

impl SemaphoreGuard {
    /// Take ownership of a freshly-created semaphore (born with one reference).
    pub(crate) fn from_born(sem: HrxSemaphore) -> Self {
        Self(sem)
    }
    pub(crate) fn as_handle(&self) -> HrxSemaphore {
        self.0
    }
}

impl Drop for SemaphoreGuard {
    fn drop(&mut self) {
        // SAFETY: we hold the one born reference; release it once.
        unsafe { hrx_semaphore_release(self.0) };
    }
}

#[no_mangle]
pub unsafe extern "C" fn hrx_semaphore_create(
    device: HrxDevice,
    initial_value: u64,
    semaphore: *mut HrxSemaphore,
) -> HrxStatus {
    if device.is_null() || semaphore.is_null() {
        return hrx_make_status(
            HrxStatusCode::InvalidArgument as i32,
            c"device or semaphore is NULL".as_ptr(),
        );
    }
    match semaphore_create(
        (*device).hal_device.as_ptr(),
        ireei::IREE_HAL_QUEUE_AFFINITY_ANY,
        initial_value,
        ireei::IREE_HAL_SEMAPHORE_FLAG_NONE,
    ) {
        Ok(hal) => {
            let device = DeviceRef::retain(device);
            *semaphore = into_handle(HrxSemaphoreS { hal, device });
            hrx_ok_status()
        }
        Err(s) => hrx_status_from_iree(s),
    }
}

#[no_mangle]
pub unsafe extern "C" fn hrx_semaphore_retain(semaphore: HrxSemaphore) {
    handle_retain(semaphore);
}

#[no_mangle]
pub unsafe extern "C" fn hrx_semaphore_release(semaphore: HrxSemaphore) {
    handle_release(semaphore);
}

#[no_mangle]
pub unsafe extern "C" fn hrx_semaphore_query(semaphore: HrxSemaphore, value: *mut u64) -> HrxStatus {
    if semaphore.is_null() || value.is_null() {
        return hrx_make_status(
            HrxStatusCode::InvalidArgument as i32,
            c"semaphore or value is NULL".as_ptr(),
        );
    }
    // Pass the caller's storage straight through, exactly like C (IREE writes it
    // regardless of status).
    hrx_status_from_iree(handle_ref(semaphore).hal.query_into(&mut *value))
}

#[no_mangle]
pub unsafe extern "C" fn hrx_semaphore_wait(
    semaphore: HrxSemaphore,
    value: u64,
    timeout_ns: u64,
) -> HrxStatus {
    if semaphore.is_null() {
        return hrx_make_status(HrxStatusCode::InvalidArgument as i32, c"semaphore is NULL".as_ptr());
    }
    let timeout = if timeout_ns == u64::MAX {
        ireei::iree_timeout_t::infinite()
    } else if timeout_ns == 0 {
        ireei::iree_timeout_t::immediate()
    } else {
        ireei::iree_timeout_t::relative_ns(timeout_ns as i64)
    };
    hrx_status_from_iree(handle_ref(semaphore).hal.wait(value, timeout))
}

#[no_mangle]
pub unsafe extern "C" fn hrx_semaphore_signal(semaphore: HrxSemaphore, value: u64) -> HrxStatus {
    if semaphore.is_null() {
        return hrx_make_status(HrxStatusCode::InvalidArgument as i32, c"semaphore is NULL".as_ptr());
    }
    hrx_status_from_iree(handle_ref(semaphore).hal.signal(value))
}
