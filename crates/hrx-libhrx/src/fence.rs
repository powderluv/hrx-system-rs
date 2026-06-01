//! Rust port of libhrx/src/libhrx/fence.c — timeline fence wrapper.
#![allow(non_snake_case)]

use core::ffi::c_void;
use core::sync::atomic::{AtomicI32, Ordering};

use crate::common::*;
use crate::semaphore::HrxSemaphore;
use iree_sys as iree;
use iree_sys::fem;
use iree_sys::init as ireei;

/// `hrx_fence_s` = { ref_count, hal_fence }.
#[repr(C)]
pub struct HrxFenceS {
    pub ref_count: AtomicI32,
    pub hal_fence: *mut fem::iree_hal_fence_t,
}
pub type HrxFence = *mut HrxFenceS;

unsafe fn alloc_fence() -> *mut HrxFenceS {
    libc::calloc(1, core::mem::size_of::<HrxFenceS>()) as *mut HrxFenceS
}

#[no_mangle]
pub unsafe extern "C" fn hrx_fence_create(capacity: usize, fence: *mut HrxFence) -> HrxStatus {
    if fence.is_null() {
        return hrx_make_status(HrxStatusCode::InvalidArgument as i32, c"fence is NULL".as_ptr());
    }
    *fence = core::ptr::null_mut();
    let created = alloc_fence();
    if created.is_null() {
        return hrx_make_status(HrxStatusCode::OutOfMemory as i32, c"failed to allocate fence".as_ptr());
    }
    let mut hal: *mut fem::iree_hal_fence_t = core::ptr::null_mut();
    let s = fem::iree_hal_fence_create(capacity, iree::allocator_system(), &mut hal);
    if !iree::status_is_ok(s) {
        libc::free(created as *mut c_void);
        return hrx_status_from_iree(s);
    }
    (*created).ref_count = AtomicI32::new(1);
    (*created).hal_fence = hal;
    *fence = created;
    hrx_ok_status()
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
    let created = alloc_fence();
    if created.is_null() {
        return hrx_make_status(HrxStatusCode::OutOfMemory as i32, c"failed to allocate fence".as_ptr());
    }
    let mut hal: *mut fem::iree_hal_fence_t = core::ptr::null_mut();
    let s = fem::iree_hal_fence_create_at(
        (*semaphore).hal_semaphore,
        value,
        iree::allocator_system(),
        &mut hal,
    );
    if !iree::status_is_ok(s) {
        libc::free(created as *mut c_void);
        return hrx_status_from_iree(s);
    }
    (*created).ref_count = AtomicI32::new(1);
    (*created).hal_fence = hal;
    *fence = created;
    hrx_ok_status()
}

#[no_mangle]
pub unsafe extern "C" fn hrx_fence_retain(fence: HrxFence) {
    fem::iree_hal_fence_retain((*fence).hal_fence);
    (*fence).ref_count.fetch_add(1, Ordering::Relaxed);
}

#[no_mangle]
pub unsafe extern "C" fn hrx_fence_release(fence: HrxFence) {
    fem::iree_hal_fence_release((*fence).hal_fence);
    if (*fence).ref_count.fetch_sub(1, Ordering::AcqRel) == 1 {
        libc::free(fence as *mut c_void);
    }
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
    hrx_status_from_iree(fem::iree_hal_fence_insert(
        (*fence).hal_fence,
        (*semaphore).hal_semaphore,
        value,
    ))
}

#[no_mangle]
pub unsafe extern "C" fn hrx_fence_extend(into_fence: HrxFence, from_fence: HrxFence) -> HrxStatus {
    if into_fence.is_null() || from_fence.is_null() {
        return hrx_make_status(
            HrxStatusCode::InvalidArgument as i32,
            c"into_fence or from_fence is NULL".as_ptr(),
        );
    }
    hrx_status_from_iree(fem::iree_hal_fence_extend(
        (*into_fence).hal_fence,
        (*from_fence).hal_fence,
    ))
}

#[no_mangle]
pub unsafe extern "C" fn hrx_fence_signal(fence: HrxFence) -> HrxStatus {
    if fence.is_null() {
        return hrx_make_status(HrxStatusCode::InvalidArgument as i32, c"fence is NULL".as_ptr());
    }
    hrx_status_from_iree(fem::iree_hal_fence_signal((*fence).hal_fence))
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
    hrx_status_from_iree(fem::iree_hal_fence_wait((*fence).hal_fence, timeout, 0))
}
