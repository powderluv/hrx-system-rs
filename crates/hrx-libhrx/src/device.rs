//! Rust port of libhrx/src/libhrx/device.c — device ops, plus the device/
//! allocator structs from hrx_internal.h. Layout matches the C structs because
//! callers receive raw `hrx_device_t` pointers into this storage.
#![allow(non_snake_case)]

use core::ffi::{c_char, c_int, c_void};
use core::sync::atomic::AtomicI32;

use crate::common::*;
use iree_hal::{HalAllocator, HalDevice, HalDeviceGroup};
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

/// Inline allocator (hrx_allocator_s) — owned by the device. The `hal_allocator`
/// is an RAII `HalAllocator` (released when the device drops). The `device`
/// back-pointer is used by the allocator API (hrx_allocator_retain/release/
/// allocate_buffer); the dead `ref_count` is kept only to mirror the C field set.
pub struct HrxAllocatorInline {
    pub ref_count: AtomicI32,
    pub hal_allocator: HalAllocator,
    pub device: HrxDevice,
}

/// hrx_device_s — the internal device object behind the opaque `hrx_device_t`.
///
/// Phase-2 owned model: the handle is the data pointer of an `Arc<HrxDeviceS>`;
/// the accelerator's `devices` Vec holds the one creation reference,
/// `hrx_device_get` returns a *borrowed* pointer (no retain), child objects
/// retain/release via the public fns (now `Arc` refcount ops). The HAL handles
/// are RAII wrappers, so teardown is just field drop in declaration order —
/// `allocator` (HalAllocator) → `hal_device_group` → `hal_device`, matching the C
/// release order — with no explicit `Drop`. The struct stays at a stable address
/// (the inline allocator's interior pointer + back-reference depend on it, built
/// via `Arc::new_cyclic`). C never frees the struct (fixed-array model); the
/// `Arc` freeing it on last drop is not observable in valid use (fixes the leak).
pub struct HrxDeviceS {
    pub type_: i32,
    pub ordinal: i32,
    // Drop order is load-bearing: allocator first, then group, then device.
    pub allocator: HrxAllocatorInline,
    pub hal_device_group: HalDeviceGroup,
    pub hal_device: HalDevice,
    pub name: [c_char; 128],
    pub architecture: [c_char; 64],
}

pub type HrxDevice = *mut HrxDeviceS;

/// An owned reference to a (not-yet-migrated) `hrx_device_t`. Constructing it
/// retains the device; `Drop` releases it. This lets a migrated child object
/// (semaphore, …) hold exactly one device reference for its lifetime via RAII,
/// bridging until the device itself moves to the owned model in Phase 2. Place it
/// after the child's IREE wrapper in the struct so it drops second (child IREE
/// object released first, then the device — matching the C release order).
pub(crate) struct DeviceRef(HrxDevice);

impl DeviceRef {
    /// Retain `device` and take an owned reference.
    ///
    /// # Safety
    /// `device` must be a live `hrx_device_t`.
    pub(crate) unsafe fn retain(device: HrxDevice) -> Self {
        hrx_device_retain(device);
        Self(device)
    }
    pub(crate) fn as_ptr(&self) -> HrxDevice {
        self.0
    }
}

impl Drop for DeviceRef {
    fn drop(&mut self) {
        // SAFETY: we hold one reference taken in `retain`; release it once.
        unsafe { hrx_device_release(self.0) };
    }
}

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
                dev.hal_device.as_ptr(),
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
        dev.hal_device.as_ptr(),
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
    crate::handle::handle_retain(device);
}

#[no_mangle]
pub unsafe extern "C" fn hrx_device_release(device: HrxDevice) {
    // The HAL teardown moved into `HrxDeviceS::drop`, which runs on the last
    // reference. (The C code released `hal_device` on every call to balance a
    // per-retain `iree_hal_device_retain`; the owned model holds one HAL device
    // reference for the lifetime and releases it once on drop — observably
    // equivalent.)
    crate::handle::handle_release(device);
}
