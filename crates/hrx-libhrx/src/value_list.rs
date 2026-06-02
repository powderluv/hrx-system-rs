//! Rust port of libhrx/src/libhrx/value_list.c — i64, null-ref, and the
//! buffer/buffer_view/fence ref-pushes.
//!
//! Phase-2 owned model: the opaque `hrx_value_list_t` is the `Arc` data pointer of
//! an `HrxValueListS`. retain/release are `Arc` refcount ops, and on the last
//! release the single `HalVmList` field drops (`iree_vm_list_release`), matching
//! the C release order (release the VM list, then free the struct).
#![allow(non_snake_case)]

use crate::buffer::{buffer_hal, HrxBuffer};
use crate::buffer_view::HrxBufferView;
use crate::common::*;
use crate::fence::HrxFence;
use crate::handle::{handle_ref, handle_release, handle_retain, into_handle};
use iree_hal::HalVmList;
use iree_sys as iree;
use iree_sys::fem;

/// `struct hrx_value_list_s` — the object behind the opaque `hrx_value_list_t`.
/// The single `vm_list` RAII field releases the VM list on the last drop.
pub struct HrxValueListS {
    vm_list: HalVmList,
}

pub type HrxValueList = *mut HrxValueListS;

/// Borrow the raw IREE VM list pointer behind a handle (for `hrx_function_invoke`,
/// which passes the args/rets lists to `iree_vm_invoke`).
///
/// # Safety
/// `list` must be a live `hrx_value_list_t`.
pub(crate) unsafe fn value_list_vm(list: HrxValueList) -> *mut iree::iree_vm_list_t {
    handle_ref(list).vm_list.as_ptr()
}

#[no_mangle]
pub unsafe extern "C" fn hrx_value_list_create(
    capacity: usize,
    list: *mut HrxValueList,
) -> HrxStatus {
    if list.is_null() {
        return hrx_make_status(
            HrxStatusCode::InvalidArgument as i32,
            c"list is NULL".as_ptr(),
        );
    }
    *list = core::ptr::null_mut();

    let mut vm_list: *mut iree::iree_vm_list_t = core::ptr::null_mut();
    let status = iree::iree_vm_list_create(
        iree::IREE_VM_TYPE_DEF_UNDEFINED,
        capacity,
        iree::allocator_system(),
        &mut vm_list,
    );
    if !iree::status_is_ok(status) {
        return hrx_status_from_iree(status);
    }

    let vm_list = HalVmList::from_owned(vm_list).expect("OK create with null vm_list");
    *list = into_handle(HrxValueListS { vm_list });
    hrx_ok_status()
}

#[no_mangle]
pub unsafe extern "C" fn hrx_value_list_retain(list: HrxValueList) {
    handle_retain(list);
}

#[no_mangle]
pub unsafe extern "C" fn hrx_value_list_release(list: HrxValueList) {
    // The VM list release moved into the `vm_list` field drop, which runs on the
    // last reference. C released the VM list on every call to balance per-retain
    // retains; the owned model holds one reference and releases it once on drop —
    // observably equivalent.
    handle_release(list);
}

#[no_mangle]
pub unsafe extern "C" fn hrx_value_list_size(list: HrxValueList, size: *mut usize) -> HrxStatus {
    if list.is_null() || size.is_null() {
        return hrx_make_status(
            HrxStatusCode::InvalidArgument as i32,
            c"list or size is NULL".as_ptr(),
        );
    }
    *size = iree::iree_vm_list_size(handle_ref(list).vm_list.as_ptr());
    hrx_ok_status()
}

#[no_mangle]
pub unsafe extern "C" fn hrx_value_list_push_i64(list: HrxValueList, value: i64) -> HrxStatus {
    if list.is_null() {
        return hrx_make_status(
            HrxStatusCode::InvalidArgument as i32,
            c"list is NULL".as_ptr(),
        );
    }
    let vm_value = iree::iree_vm_value_t::make_i64(value);
    hrx_status_from_iree(iree::iree_vm_list_push_value(handle_ref(list).vm_list.as_ptr(), &vm_value))
}

#[no_mangle]
pub unsafe extern "C" fn hrx_value_list_push_null_ref(list: HrxValueList) -> HrxStatus {
    if list.is_null() {
        return hrx_make_status(
            HrxStatusCode::InvalidArgument as i32,
            c"list is NULL".as_ptr(),
        );
    }
    let mut r = iree::iree_vm_ref_t::null();
    hrx_status_from_iree(iree::iree_vm_list_push_ref_move(handle_ref(list).vm_list.as_ptr(), &mut r))
}

#[no_mangle]
pub unsafe extern "C" fn hrx_value_list_push_buffer(
    list: HrxValueList,
    buffer: HrxBuffer,
) -> HrxStatus {
    if list.is_null() || buffer.is_null() {
        return hrx_make_status(
            HrxStatusCode::InvalidArgument as i32,
            c"list or buffer is NULL".as_ptr(),
        );
    }
    let mut r = fem::iree_hal_buffer_retain_ref(buffer_hal(buffer));
    hrx_status_from_iree(iree::iree_vm_list_push_ref_move(handle_ref(list).vm_list.as_ptr(), &mut r))
}

#[no_mangle]
pub unsafe extern "C" fn hrx_value_list_push_buffer_view(
    list: HrxValueList,
    buffer_view: HrxBufferView,
) -> HrxStatus {
    if list.is_null() || buffer_view.is_null() {
        return hrx_make_status(
            HrxStatusCode::InvalidArgument as i32,
            c"list or buffer_view is NULL".as_ptr(),
        );
    }
    let mut r = fem::iree_hal_buffer_view_retain_ref(crate::buffer_view::buffer_view_hal_ptr(buffer_view));
    hrx_status_from_iree(iree::iree_vm_list_push_ref_move(handle_ref(list).vm_list.as_ptr(), &mut r))
}

#[no_mangle]
pub unsafe extern "C" fn hrx_value_list_push_fence(
    list: HrxValueList,
    fence: HrxFence,
) -> HrxStatus {
    if list.is_null() || fence.is_null() {
        return hrx_make_status(
            HrxStatusCode::InvalidArgument as i32,
            c"list or fence is NULL".as_ptr(),
        );
    }
    let mut r = fem::iree_hal_fence_retain_ref(crate::fence::fence_hal_ptr(fence));
    hrx_status_from_iree(iree::iree_vm_list_push_ref_move(handle_ref(list).vm_list.as_ptr(), &mut r))
}

#[no_mangle]
pub unsafe extern "C" fn hrx_value_list_get_i64(
    list: HrxValueList,
    index: usize,
    value: *mut i64,
) -> HrxStatus {
    if list.is_null() || value.is_null() {
        return hrx_make_status(
            HrxStatusCode::InvalidArgument as i32,
            c"list or value is NULL".as_ptr(),
        );
    }
    let mut vm_value = iree::iree_vm_value_t { type_: 0, _pad: 0, storage: [0; 8] };
    let status = iree::iree_vm_list_get_value_as(
        handle_ref(list).vm_list.as_ptr(),
        index,
        iree::IREE_VM_VALUE_TYPE_I64,
        &mut vm_value,
    );
    if !iree::status_is_ok(status) {
        return hrx_status_from_iree(status);
    }
    *value = vm_value.as_i64();
    hrx_ok_status()
}
