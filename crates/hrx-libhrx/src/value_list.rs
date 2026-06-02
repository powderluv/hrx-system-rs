//! Rust port of libhrx/src/libhrx/value_list.c — i64, null-ref, and the
//! buffer/buffer_view/fence ref-pushes.
#![allow(non_snake_case)]

use core::ffi::c_void;
use core::sync::atomic::{AtomicI32, Ordering};

use crate::buffer::{buffer_hal, HrxBuffer};
use crate::buffer_view::HrxBufferView;
use crate::common::*;
use crate::fence::HrxFence;
use iree_sys as iree;
use iree_sys::fem;

/// `struct hrx_value_list_s` — { ref_count (atomic i32), vm_list ptr }. Layout
/// matches the C struct (iree_atomic_ref_count_t is an atomic int32).
#[repr(C)]
pub struct HrxValueListS {
    pub ref_count: AtomicI32,
    pub vm_list: *mut iree::iree_vm_list_t,
}

pub type HrxValueList = *mut HrxValueListS;

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

    let created = libc::calloc(1, core::mem::size_of::<HrxValueListS>()) as *mut HrxValueListS;
    if created.is_null() {
        return hrx_make_status(
            HrxStatusCode::OutOfMemory as i32,
            c"failed to allocate value list".as_ptr(),
        );
    }

    let mut vm_list: *mut iree::iree_vm_list_t = core::ptr::null_mut();
    let status = iree::iree_vm_list_create(
        iree::IREE_VM_TYPE_DEF_UNDEFINED,
        capacity,
        iree::allocator_system(),
        &mut vm_list,
    );
    if !iree::status_is_ok(status) {
        libc::free(created as *mut c_void);
        return hrx_status_from_iree(status);
    }

    (*created).vm_list = vm_list;
    // iree_atomic_ref_count_init -> value 1.
    (*created).ref_count.store(1, Ordering::Relaxed);
    *list = created;
    hrx_ok_status()
}

#[no_mangle]
pub unsafe extern "C" fn hrx_value_list_retain(list: HrxValueList) {
    iree::iree_vm_list_retain((*list).vm_list);
    (*list).ref_count.fetch_add(1, Ordering::Relaxed);
}

#[no_mangle]
pub unsafe extern "C" fn hrx_value_list_release(list: HrxValueList) {
    iree::iree_vm_list_release((*list).vm_list);
    // iree_atomic_ref_count_dec returns the PREVIOUS value; ==1 means this was
    // the last reference.
    if (*list).ref_count.fetch_sub(1, Ordering::AcqRel) == 1 {
        libc::free(list as *mut c_void);
    }
}

#[no_mangle]
pub unsafe extern "C" fn hrx_value_list_size(list: HrxValueList, size: *mut usize) -> HrxStatus {
    if list.is_null() || size.is_null() {
        return hrx_make_status(
            HrxStatusCode::InvalidArgument as i32,
            c"list or size is NULL".as_ptr(),
        );
    }
    *size = iree::iree_vm_list_size((*list).vm_list);
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
    hrx_status_from_iree(iree::iree_vm_list_push_value((*list).vm_list, &vm_value))
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
    hrx_status_from_iree(iree::iree_vm_list_push_ref_move((*list).vm_list, &mut r))
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
    hrx_status_from_iree(iree::iree_vm_list_push_ref_move((*list).vm_list, &mut r))
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
    hrx_status_from_iree(iree::iree_vm_list_push_ref_move((*list).vm_list, &mut r))
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
    hrx_status_from_iree(iree::iree_vm_list_push_ref_move((*list).vm_list, &mut r))
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
        (*list).vm_list,
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
