//! Rust port of libhrx/src/libhrx/semaphore.c — timeline semaphore ops.
#![allow(non_snake_case)]

use core::ffi::c_void;
use core::sync::atomic::{AtomicI32, Ordering};

use crate::common::*;
use crate::device::{hrx_device_release, hrx_device_retain, HrxDevice};
use iree_sys as iree;
use iree_sys::init as ireei;

/// `hrx_semaphore_s` = { ref_count, hal_semaphore, device }.
#[repr(C)]
pub struct HrxSemaphoreS {
    pub ref_count: AtomicI32,
    pub hal_semaphore: *mut ireei::iree_hal_semaphore_t,
    pub device: HrxDevice,
}

pub type HrxSemaphore = *mut HrxSemaphoreS;

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
    let sem = libc::calloc(1, core::mem::size_of::<HrxSemaphoreS>()) as *mut HrxSemaphoreS;
    if sem.is_null() {
        return hrx_make_status(
            HrxStatusCode::OutOfMemory as i32,
            c"failed to allocate semaphore".as_ptr(),
        );
    }
    let mut hal_sem: *mut ireei::iree_hal_semaphore_t = core::ptr::null_mut();
    let s = ireei::iree_hal_semaphore_create(
        (*device).hal_device,
        ireei::IREE_HAL_QUEUE_AFFINITY_ANY,
        initial_value,
        ireei::IREE_HAL_SEMAPHORE_FLAG_NONE,
        &mut hal_sem,
    );
    if !iree::status_is_ok(s) {
        libc::free(sem as *mut c_void);
        return hrx_status_from_iree(s);
    }
    (*sem).ref_count = AtomicI32::new(1);
    (*sem).hal_semaphore = hal_sem;
    (*sem).device = device;
    hrx_device_retain((*sem).device);
    *semaphore = sem;
    hrx_ok_status()
}

#[no_mangle]
pub unsafe extern "C" fn hrx_semaphore_retain(semaphore: HrxSemaphore) {
    ireei::iree_hal_semaphore_retain((*semaphore).hal_semaphore);
    hrx_device_retain((*semaphore).device);
    (*semaphore).ref_count.fetch_add(1, Ordering::Relaxed);
}

#[no_mangle]
pub unsafe extern "C" fn hrx_semaphore_release(semaphore: HrxSemaphore) {
    ireei::iree_hal_semaphore_release((*semaphore).hal_semaphore);
    hrx_device_release((*semaphore).device);
    if (*semaphore).ref_count.fetch_sub(1, Ordering::AcqRel) == 1 {
        libc::free(semaphore as *mut c_void);
    }
}

#[no_mangle]
pub unsafe extern "C" fn hrx_semaphore_query(semaphore: HrxSemaphore, value: *mut u64) -> HrxStatus {
    if semaphore.is_null() || value.is_null() {
        return hrx_make_status(
            HrxStatusCode::InvalidArgument as i32,
            c"semaphore or value is NULL".as_ptr(),
        );
    }
    hrx_status_from_iree(ireei::iree_hal_semaphore_query((*semaphore).hal_semaphore, value))
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
    hrx_status_from_iree(ireei::iree_hal_semaphore_wait(
        (*semaphore).hal_semaphore,
        value,
        timeout,
        0,
    ))
}

#[no_mangle]
pub unsafe extern "C" fn hrx_semaphore_signal(semaphore: HrxSemaphore, value: u64) -> HrxStatus {
    if semaphore.is_null() {
        return hrx_make_status(HrxStatusCode::InvalidArgument as i32, c"semaphore is NULL".as_ptr());
    }
    hrx_status_from_iree(ireei::iree_hal_semaphore_signal(
        (*semaphore).hal_semaphore,
        value,
        core::ptr::null_mut(),
    ))
}
