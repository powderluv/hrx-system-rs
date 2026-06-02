//! Rust port of libhrx/src/libhrx/buffer_view.c — buffer view metadata.
//!
//! Phase-1 owned-object model: the opaque `hrx_buffer_view_t` is the `Arc` data
//! pointer of an `HrxBufferViewS`; retain/release are `Arc` refcount ops and the
//! IREE buffer view is released exactly once by `HalBufferView`'s `Drop`. The view
//! has no parent reference to manage — the IREE view internally holds the IREE
//! buffer alive, matching the C code (which does not retain the hrx buffer).
#![allow(non_snake_case)]

use crate::buffer::{buffer_hal, HrxBuffer};
use crate::common::*;
use crate::handle::{handle_ref, handle_release, handle_retain, into_handle};
use iree_hal::{buffer_view_create, HalBufferView};
use iree_sys::fem;

/// Internal object behind the opaque `hrx_buffer_view_t`.
pub struct HrxBufferViewS {
    hal: HalBufferView,
}
pub type HrxBufferView = *mut HrxBufferViewS;

/// Borrow the raw IREE buffer-view pointer behind a handle (for value_list's
/// vm-ref adapter).
///
/// # Safety
/// `bv` must be a live `hrx_buffer_view_t`.
pub(crate) unsafe fn buffer_view_hal_ptr(bv: HrxBufferView) -> *mut fem::iree_hal_buffer_view_t {
    handle_ref(bv).hal.as_ptr()
}

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

    // Stack buffer for shapes up to 8 dims (matches C); heap for larger. The
    // chosen storage must outlive the create call below.
    let mut stack_shape = [0u64; 8];
    let mut heap_shape: Vec<u64> = Vec::new();
    let iree_shape: *const u64 = if shape_rank > 8 {
        heap_shape.reserve(shape_rank);
        for i in 0..shape_rank {
            let dim = *shape.add(i);
            if dim < 0 {
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
                return hrx_make_status(
                    HrxStatusCode::InvalidArgument as i32,
                    c"shape dimensions must be non-negative".as_ptr(),
                );
            }
            stack_shape[i] = dim as u64;
        }
        stack_shape.as_ptr()
    };

    match buffer_view_create(
        buffer_hal(buffer),
        shape_rank,
        iree_shape,
        element_type,
        encoding_type,
    ) {
        Ok(hal) => {
            *buffer_view = into_handle(HrxBufferViewS { hal });
            hrx_ok_status()
        }
        Err(s) => hrx_status_from_iree(s),
    }
}

#[no_mangle]
pub unsafe extern "C" fn hrx_buffer_view_retain(buffer_view: HrxBufferView) {
    handle_retain(buffer_view);
}

#[no_mangle]
pub unsafe extern "C" fn hrx_buffer_view_release(buffer_view: HrxBufferView) {
    handle_release(buffer_view);
}

#[no_mangle]
pub unsafe extern "C" fn hrx_buffer_view_rank(
    buffer_view: HrxBufferView,
    rank: *mut usize,
) -> HrxStatus {
    if buffer_view.is_null() || rank.is_null() {
        return hrx_make_status(
            HrxStatusCode::InvalidArgument as i32,
            c"buffer_view or rank is NULL".as_ptr(),
        );
    }
    *rank = handle_ref(buffer_view).hal.shape_rank();
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
    let view = handle_ref(buffer_view);
    if dim >= view.hal.shape_rank() {
        return hrx_make_status(
            HrxStatusCode::OutOfRange as i32,
            c"buffer_view dim out of range".as_ptr(),
        );
    }
    *value = view.hal.shape_dim(dim) as i64;
    hrx_ok_status()
}
