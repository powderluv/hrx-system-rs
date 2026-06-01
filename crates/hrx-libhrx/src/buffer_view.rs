//! Rust port of libhrx/src/libhrx/buffer_view.c — buffer view metadata wrapper.
#![allow(non_snake_case)]

use core::ffi::c_void;
use core::sync::atomic::{AtomicI32, Ordering};

use crate::buffer::HrxBuffer;
use crate::common::*;
use iree_sys as iree;
use iree_sys::fem;

/// `hrx_buffer_view_s` = { ref_count, hal_buffer_view }.
#[repr(C)]
pub struct HrxBufferViewS {
    pub ref_count: AtomicI32,
    pub hal_buffer_view: *mut fem::iree_hal_buffer_view_t,
}
pub type HrxBufferView = *mut HrxBufferViewS;

#[no_mangle]
pub unsafe extern "C" fn hrx_buffer_view_create(
    buffer: HrxBuffer,
    shape_rank: usize,
    shape: *const i64,
    element_type: u32,
    encoding_type: u32,
    buffer_view: *mut HrxBufferView,
) -> HrxStatus {
    if buffer.is_null() || buffer_view.is_null() {
        return hrx_make_status(
            HrxStatusCode::InvalidArgument as i32,
            c"buffer or buffer_view is NULL".as_ptr(),
        );
    }
    *buffer_view = core::ptr::null_mut();
    if shape_rank > 0 && shape.is_null() {
        return hrx_make_status(
            HrxStatusCode::InvalidArgument as i32,
            c"shape is NULL for non-zero rank".as_ptr(),
        );
    }

    let created = libc::calloc(1, core::mem::size_of::<HrxBufferViewS>()) as *mut HrxBufferViewS;
    if created.is_null() {
        return hrx_make_status(HrxStatusCode::OutOfMemory as i32, c"failed to allocate buffer_view".as_ptr());
    }

    // Stack buffer for shapes up to 8 dims (matches C); heap for larger.
    let mut stack_shape = [0u64; 8];
    let mut heap_shape: Vec<u64> = Vec::new();
    let iree_shape: *const u64 = if shape_rank > 8 {
        heap_shape.reserve(shape_rank);
        for i in 0..shape_rank {
            let dim = *shape.add(i);
            if dim < 0 {
                libc::free(created as *mut c_void);
                return hrx_make_status(
                    HrxStatusCode::InvalidArgument as i32,
                    c"shape dimensions must be non-negative".as_ptr(),
                );
            }
            heap_shape.push(dim as u64);
        }
        heap_shape.as_ptr()
    } else {
        for i in 0..shape_rank {
            let dim = *shape.add(i);
            if dim < 0 {
                libc::free(created as *mut c_void);
                return hrx_make_status(
                    HrxStatusCode::InvalidArgument as i32,
                    c"shape dimensions must be non-negative".as_ptr(),
                );
            }
            stack_shape[i] = dim as u64;
        }
        stack_shape.as_ptr()
    };

    let s = fem::iree_hal_buffer_view_create(
        (*buffer).hal_buffer,
        shape_rank,
        iree_shape,
        element_type,
        encoding_type,
        iree::allocator_system(),
        &mut (*created).hal_buffer_view,
    );
    if !iree::status_is_ok(s) {
        libc::free(created as *mut c_void);
        return hrx_status_from_iree(s);
    }
    (*created).ref_count = AtomicI32::new(1);
    *buffer_view = created;
    hrx_ok_status()
}

#[no_mangle]
pub unsafe extern "C" fn hrx_buffer_view_retain(buffer_view: HrxBufferView) {
    fem::iree_hal_buffer_view_retain((*buffer_view).hal_buffer_view);
    (*buffer_view).ref_count.fetch_add(1, Ordering::Relaxed);
}

#[no_mangle]
pub unsafe extern "C" fn hrx_buffer_view_release(buffer_view: HrxBufferView) {
    fem::iree_hal_buffer_view_release((*buffer_view).hal_buffer_view);
    if (*buffer_view).ref_count.fetch_sub(1, Ordering::AcqRel) == 1 {
        libc::free(buffer_view as *mut c_void);
    }
}

#[no_mangle]
pub unsafe extern "C" fn hrx_buffer_view_rank(buffer_view: HrxBufferView, rank: *mut usize) -> HrxStatus {
    if buffer_view.is_null() || rank.is_null() {
        return hrx_make_status(
            HrxStatusCode::InvalidArgument as i32,
            c"buffer_view or rank is NULL".as_ptr(),
        );
    }
    *rank = fem::iree_hal_buffer_view_shape_rank((*buffer_view).hal_buffer_view);
    hrx_ok_status()
}

#[no_mangle]
pub unsafe extern "C" fn hrx_buffer_view_dim(
    buffer_view: HrxBufferView,
    dim: usize,
    value: *mut i64,
) -> HrxStatus {
    if buffer_view.is_null() || value.is_null() {
        return hrx_make_status(
            HrxStatusCode::InvalidArgument as i32,
            c"buffer_view or value is NULL".as_ptr(),
        );
    }
    let rank = fem::iree_hal_buffer_view_shape_rank((*buffer_view).hal_buffer_view);
    if dim >= rank {
        return hrx_make_status(
            HrxStatusCode::OutOfRange as i32,
            c"buffer_view dim out of range".as_ptr(),
        );
    }
    *value = fem::iree_hal_buffer_view_shape_dim((*buffer_view).hal_buffer_view, dim) as i64;
    hrx_ok_status()
}
