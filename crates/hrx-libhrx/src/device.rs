//! Rust port of libhrx/src/libhrx/device.c — device ops, plus the device/
//! allocator structs from hrx_internal.h. Layout matches the C structs because
//! callers receive raw `hrx_device_t` pointers into this storage.
#![allow(non_snake_case)]

use core::ffi::{c_char, c_int, c_void};
use core::sync::atomic::{AtomicI32, Ordering};

use crate::common::*;
use iree_sys as iree;
use iree_sys::init as ireei;

pub const HRX_ACCELERATOR_GPU: i32 = 0;
pub const HRX_ACCELERATOR_CPU: i32 = 1;

// hrx_device_property_t (hrx_runtime.h).
const HRX_DEVICE_PROPERTY_NAME: i32 = 0;
const HRX_DEVICE_PROPERTY_ARCHITECTURE: i32 = 1;
const HRX_DEVICE_PROPERTY_TOTAL_MEMORY: i32 = 2;
const HRX_DEVICE_PROPERTY_COMPUTE_UNITS: i32 = 3;
const HRX_DEVICE_PROPERTY_MAX_WORKGROUP_SIZE: i32 = 4;

/// Inline allocator (hrx_allocator_s) — owned by the device. C layout:
/// { ref_count, hal_allocator, device }. The `device` back-pointer is used by
/// the allocator API (hrx_allocator_retain/release/allocate_buffer).
#[repr(C)]
pub struct HrxAllocatorInline {
    pub ref_count: AtomicI32,
    pub hal_allocator: *mut iree::iree_hal_allocator_t,
    pub device: HrxDevice,
}

/// hrx_device_s. The C layout is:
///   { ref_count, type, ordinal, hal_device, hal_device_group, profiling_active,
///     allocator (inline), name[128], architecture[64] }
/// Callers only ever hold an opaque `hrx_device_t` and pass it back to hrx_*
/// functions, so exact tail layout isn't ABI-observed by callers — but the C
/// reference and our Rust must agree internally. We keep the same field order.
#[repr(C)]
pub struct HrxDeviceS {
    pub ref_count: AtomicI32,
    pub type_: i32,
    pub ordinal: i32,
    pub hal_device: *mut iree::iree_hal_device_t,
    pub hal_device_group: *mut iree::iree_hal_device_group_t,
    pub allocator: HrxAllocatorInline,
    pub name: [c_char; 128],
    pub architecture: [c_char; 64],
}

pub type HrxDevice = *mut HrxDeviceS;

unsafe fn cstr_len(buf: &[c_char]) -> usize {
    let mut n = 0;
    while n < buf.len() && buf[n] != 0 {
        n += 1;
    }
    n
}

#[no_mangle]
pub unsafe extern "C" fn hrx_device_get_property(
    device: HrxDevice,
    prop: i32,
    value: *mut c_void,
    value_size: usize,
) -> HrxStatus {
    if device.is_null() || value.is_null() {
        return hrx_make_status(
            HrxStatusCode::InvalidArgument as i32,
            c"device or value is NULL".as_ptr(),
        );
    }
    let dev = &*device;
    match prop {
        HRX_DEVICE_PROPERTY_NAME => {
            let len = cstr_len(&dev.name);
            if value_size < len + 1 {
                return hrx_make_status(
                    HrxStatusCode::OutOfRange as i32,
                    c"buffer too small for device name".as_ptr(),
                );
            }
            core::ptr::copy_nonoverlapping(dev.name.as_ptr() as *const u8, value as *mut u8, len + 1);
            hrx_ok_status()
        }
        HRX_DEVICE_PROPERTY_ARCHITECTURE => {
            let len = cstr_len(&dev.architecture);
            if value_size < len + 1 {
                return hrx_make_status(
                    HrxStatusCode::OutOfRange as i32,
                    c"buffer too small for architecture string".as_ptr(),
                );
            }
            core::ptr::copy_nonoverlapping(
                dev.architecture.as_ptr() as *const u8,
                value as *mut u8,
                len + 1,
            );
            hrx_ok_status()
        }
        HRX_DEVICE_PROPERTY_TOTAL_MEMORY => {
            if value_size < core::mem::size_of::<u64>() {
                return hrx_make_status(
                    HrxStatusCode::OutOfRange as i32,
                    c"buffer too small for uint64_t".as_ptr(),
                );
            }
            let mut mem_size: i64 = 0;
            let s = ireei::iree_hal_device_query_i64(
                dev.hal_device,
                ireei::iree_string_view_t::cstr(c"hal.device"),
                ireei::iree_string_view_t::cstr(c"memory.total"),
                &mut mem_size,
            );
            if !iree::status_is_ok(s) {
                iree::iree_status_free(s);
                mem_size = 0;
            }
            *(value as *mut u64) = mem_size as u64;
            hrx_ok_status()
        }
        HRX_DEVICE_PROPERTY_COMPUTE_UNITS | HRX_DEVICE_PROPERTY_MAX_WORKGROUP_SIZE => {
            if value_size < core::mem::size_of::<u32>() {
                return hrx_make_status(
                    HrxStatusCode::OutOfRange as i32,
                    c"buffer too small for uint32_t".as_ptr(),
                );
            }
            *(value as *mut u32) = 0; // Not available from local-task driver.
            hrx_ok_status()
        }
        _ => hrx_make_status(
            HrxStatusCode::InvalidArgument as i32,
            c"unknown device property".as_ptr(),
        ),
    }
}

#[no_mangle]
pub unsafe extern "C" fn hrx_device_synchronize(device: HrxDevice) -> HrxStatus {
    if device.is_null() {
        return hrx_make_status(HrxStatusCode::InvalidArgument as i32, c"device is NULL".as_ptr());
    }
    let dev = &*device;
    // Deprecated no-op shim: an empty wait list returns immediately.
    let empty = ireei::iree_hal_semaphore_list_t::default();
    let s = ireei::iree_hal_device_wait_semaphores(
        dev.hal_device,
        iree::IREE_ASYNC_WAIT_MODE_ALL,
        empty,
        ireei::iree_timeout_t::infinite(),
        0,
    );
    hrx_status_from_iree(s)
}

#[no_mangle]
pub unsafe extern "C" fn hrx_device_get_type(device: HrxDevice, type_: *mut c_int) -> HrxStatus {
    if device.is_null() || type_.is_null() {
        return hrx_make_status(
            HrxStatusCode::InvalidArgument as i32,
            c"device or type is NULL".as_ptr(),
        );
    }
    *type_ = (*device).type_;
    hrx_ok_status()
}

#[no_mangle]
pub unsafe extern "C" fn hrx_device_retain(device: HrxDevice) {
    let dev = &*device;
    ireei::iree_hal_device_retain(dev.hal_device);
    dev.ref_count.fetch_add(1, Ordering::Relaxed);
}

#[no_mangle]
pub unsafe extern "C" fn hrx_device_release(device: HrxDevice) {
    let dev = &mut *device;
    let hal_device = dev.hal_device;
    let hal_device_group = dev.hal_device_group;
    if dev.ref_count.fetch_sub(1, Ordering::AcqRel) == 1 {
        ireei::iree_hal_allocator_release(dev.allocator.hal_allocator);
        ireei::iree_hal_device_group_release(hal_device_group);
        dev.allocator.hal_allocator = core::ptr::null_mut();
        dev.hal_device = core::ptr::null_mut();
        dev.hal_device_group = core::ptr::null_mut();
    }
    ireei::iree_hal_device_release(hal_device);
}
