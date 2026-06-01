//! Rust port of libhrx/src/libhrx/stream.c — streams own a timeline semaphore +
//! a pending one-shot command buffer; ops accumulate and flush on demand.
#![allow(non_snake_case)]

use core::ffi::c_void;
use core::sync::atomic::{AtomicI32, Ordering};

use crate::buffer::HrxBuffer;
use crate::common::*;
use crate::device::{hrx_device_release, hrx_device_retain, HrxDevice};
use crate::executable::HrxExecutable;
use crate::queue_ops::{build_hal_bindings, HrxBufferRef, HrxDispatchConfig};
use crate::semaphore::{
    hrx_semaphore_create, hrx_semaphore_query, hrx_semaphore_release, hrx_semaphore_retain,
    hrx_semaphore_wait, HrxSemaphore,
};
use iree_sys as iree;
use iree_sys::fem;
use iree_sys::init as ireei;

/// `hrx_stream_s` = { ref_count, device, semaphore, timepoint, pending_cb,
/// has_pending_work, flags }.
#[repr(C)]
pub struct HrxStreamS {
    pub ref_count: AtomicI32,
    pub device: HrxDevice,
    pub semaphore: HrxSemaphore,
    pub timepoint: u64,
    pub pending_cb: *mut ireei::iree_hal_command_buffer_t,
    pub has_pending_work: bool,
    pub flags: u32,
}

pub type HrxStream = *mut HrxStreamS;

/// `hrx_timeline_point_t` (public) = { hrx_semaphore_t semaphore; uint64_t value }.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct HrxTimelinePoint {
    pub semaphore: HrxSemaphore,
    pub value: u64,
}

unsafe fn stream_begin_cb(stream: HrxStream) -> HrxStatus {
    if !(*stream).pending_cb.is_null() {
        return hrx_ok_status();
    }
    let mut cb: *mut ireei::iree_hal_command_buffer_t = core::ptr::null_mut();
    let s = ireei::iree_hal_command_buffer_create(
        (*(*stream).device).hal_device,
        ireei::IREE_HAL_COMMAND_BUFFER_MODE_ONE_SHOT,
        ireei::IREE_HAL_COMMAND_CATEGORY_TRANSFER | ireei::IREE_HAL_COMMAND_CATEGORY_DISPATCH,
        ireei::IREE_HAL_QUEUE_AFFINITY_ANY,
        0,
        &mut cb,
    );
    if !iree::status_is_ok(s) {
        return hrx_status_from_iree(s);
    }
    let s = ireei::iree_hal_command_buffer_begin(cb);
    if !iree::status_is_ok(s) {
        ireei::iree_hal_command_buffer_release(cb);
        (*stream).pending_cb = core::ptr::null_mut();
        return hrx_status_from_iree(s);
    }
    (*stream).pending_cb = cb;
    hrx_ok_status()
}

unsafe fn record_ordering_barrier(stream: HrxStream) -> iree::iree_status_t {
    let barrier = ireei::iree_hal_memory_barrier_t {
        source_scope: ireei::IREE_HAL_MEMORY_ACCESS_ALL as u32,
        target_scope: ireei::IREE_HAL_MEMORY_ACCESS_ALL as u32,
    };
    ireei::iree_hal_command_buffer_execution_barrier(
        (*stream).pending_cb,
        ireei::IREE_HAL_EXECUTION_STAGE_COMMAND_RETIRE,
        ireei::IREE_HAL_EXECUTION_STAGE_COMMAND_ISSUE,
        ireei::IREE_HAL_EXECUTION_BARRIER_FLAG_NONE,
        1,
        &barrier,
        0,
        core::ptr::null(),
    )
}

#[no_mangle]
pub unsafe extern "C" fn hrx_stream_create(
    device: HrxDevice,
    flags: u32,
    stream: *mut HrxStream,
) -> HrxStatus {
    if device.is_null() || stream.is_null() {
        return hrx_make_status(
            HrxStatusCode::InvalidArgument as i32,
            c"device or stream is NULL".as_ptr(),
        );
    }
    let s = libc::calloc(1, core::mem::size_of::<HrxStreamS>()) as *mut HrxStreamS;
    if s.is_null() {
        return hrx_make_status(HrxStatusCode::OutOfMemory as i32, c"failed to allocate stream".as_ptr());
    }
    (*s).ref_count = AtomicI32::new(1);
    (*s).device = device;
    hrx_device_retain((*s).device);
    (*s).flags = flags;
    (*s).timepoint = 0;
    (*s).has_pending_work = false;
    (*s).pending_cb = core::ptr::null_mut();

    let mut sem: HrxSemaphore = core::ptr::null_mut();
    let st = hrx_semaphore_create(device, 0, &mut sem);
    if !hrx_status_is_ok(st) {
        libc::free(s as *mut c_void);
        return st;
    }
    (*s).semaphore = sem;
    *stream = s;
    hrx_ok_status()
}

#[no_mangle]
pub unsafe extern "C" fn hrx_stream_retain(stream: HrxStream) {
    hrx_device_retain((*stream).device);
    hrx_semaphore_retain((*stream).semaphore);
    (*stream).ref_count.fetch_add(1, Ordering::Relaxed);
}

#[no_mangle]
pub unsafe extern "C" fn hrx_stream_release(stream: HrxStream) {
    let device = (*stream).device;
    let semaphore = (*stream).semaphore;
    if (*stream).ref_count.fetch_sub(1, Ordering::AcqRel) == 1 {
        if (*stream).has_pending_work {
            let s = hrx_stream_flush(stream);
            crate::common::hrx_status_drop(s);
        }
        if (*stream).timepoint > 0 {
            let s = hrx_semaphore_wait((*stream).semaphore, (*stream).timepoint, u64::MAX);
            crate::common::hrx_status_drop(s);
        }
        if !(*stream).pending_cb.is_null() {
            ireei::iree_hal_command_buffer_release((*stream).pending_cb);
        }
        libc::free(stream as *mut c_void);
    }
    hrx_semaphore_release(semaphore);
    hrx_device_release(device);
}

#[no_mangle]
pub unsafe extern "C" fn hrx_stream_flush(stream: HrxStream) -> HrxStatus {
    if stream.is_null() {
        return hrx_make_status(HrxStatusCode::InvalidArgument as i32, c"stream is NULL".as_ptr());
    }
    if !(*stream).has_pending_work || (*stream).pending_cb.is_null() {
        return hrx_ok_status();
    }
    let s = ireei::iree_hal_command_buffer_end((*stream).pending_cb);
    if !iree::status_is_ok(s) {
        return hrx_status_from_iree(s);
    }
    let mut wait_value = (*stream).timepoint;
    let mut signal_value = (*stream).timepoint + 1;
    let mut sem = (*(*stream).semaphore).hal_semaphore;
    let wait_list = ireei::iree_hal_semaphore_list_t {
        count: if (*stream).timepoint > 0 { 1 } else { 0 },
        semaphores: &mut sem,
        payload_values: &mut wait_value,
    };
    let signal_list = ireei::iree_hal_semaphore_list_t {
        count: 1,
        semaphores: &mut sem,
        payload_values: &mut signal_value,
    };
    let binding_table = ireei::iree_hal_buffer_binding_table_t::default();
    let s = ireei::iree_hal_device_queue_execute(
        (*(*stream).device).hal_device,
        ireei::IREE_HAL_QUEUE_AFFINITY_ANY,
        wait_list,
        signal_list,
        (*stream).pending_cb,
        binding_table,
        0,
    );
    if !iree::status_is_ok(s) {
        return hrx_status_from_iree(s);
    }
    (*stream).timepoint = signal_value;
    ireei::iree_hal_command_buffer_release((*stream).pending_cb);
    (*stream).pending_cb = core::ptr::null_mut();
    (*stream).has_pending_work = false;
    hrx_ok_status()
}

#[no_mangle]
pub unsafe extern "C" fn hrx_stream_synchronize(stream: HrxStream) -> HrxStatus {
    if stream.is_null() {
        return hrx_make_status(HrxStatusCode::InvalidArgument as i32, c"stream is NULL".as_ptr());
    }
    let s = hrx_stream_flush(stream);
    if !hrx_status_is_ok(s) {
        return s;
    }
    hrx_stream_wait(stream)
}

#[no_mangle]
pub unsafe extern "C" fn hrx_stream_wait(stream: HrxStream) -> HrxStatus {
    if stream.is_null() {
        return hrx_make_status(HrxStatusCode::InvalidArgument as i32, c"stream is NULL".as_ptr());
    }
    if (*stream).timepoint == 0 {
        return hrx_ok_status();
    }
    hrx_semaphore_wait((*stream).semaphore, (*stream).timepoint, u64::MAX)
}

#[no_mangle]
pub unsafe extern "C" fn hrx_stream_query(stream: HrxStream, complete: *mut bool) -> HrxStatus {
    if stream.is_null() || complete.is_null() {
        return hrx_make_status(
            HrxStatusCode::InvalidArgument as i32,
            c"stream or complete is NULL".as_ptr(),
        );
    }
    if (*stream).timepoint == 0 {
        *complete = true;
        return hrx_ok_status();
    }
    let mut current: u64 = 0;
    let s = hrx_semaphore_query((*stream).semaphore, &mut current);
    if !hrx_status_is_ok(s) {
        return s;
    }
    *complete = current >= (*stream).timepoint;
    hrx_ok_status()
}

#[no_mangle]
pub unsafe extern "C" fn hrx_stream_get_semaphore(
    stream: HrxStream,
    semaphore: *mut HrxSemaphore,
) -> HrxStatus {
    if stream.is_null() || semaphore.is_null() {
        return hrx_make_status(
            HrxStatusCode::InvalidArgument as i32,
            c"stream or semaphore is NULL".as_ptr(),
        );
    }
    *semaphore = (*stream).semaphore;
    hrx_ok_status()
}

#[no_mangle]
pub unsafe extern "C" fn hrx_stream_get_device(stream: HrxStream, device: *mut HrxDevice) -> HrxStatus {
    if stream.is_null() || device.is_null() {
        return hrx_make_status(
            HrxStatusCode::InvalidArgument as i32,
            c"stream or device is NULL".as_ptr(),
        );
    }
    *device = (*stream).device;
    hrx_ok_status()
}

#[no_mangle]
pub unsafe extern "C" fn hrx_stream_get_timeline_position(
    stream: HrxStream,
    position: *mut HrxTimelinePoint,
) -> HrxStatus {
    if stream.is_null() || position.is_null() {
        return hrx_make_status(
            HrxStatusCode::InvalidArgument as i32,
            c"stream or position is NULL".as_ptr(),
        );
    }
    (*position).semaphore = (*stream).semaphore;
    (*position).value = (*stream).timepoint;
    hrx_ok_status()
}

#[no_mangle]
pub unsafe extern "C" fn hrx_stream_advance_timeline(stream: HrxStream, value: *mut u64) -> HrxStatus {
    if stream.is_null() || value.is_null() {
        return hrx_make_status(
            HrxStatusCode::InvalidArgument as i32,
            c"stream or value is NULL".as_ptr(),
        );
    }
    (*stream).timepoint += 1;
    *value = (*stream).timepoint;
    hrx_ok_status()
}

#[no_mangle]
pub unsafe extern "C" fn hrx_stream_wait_on(stream: HrxStream, position: HrxTimelinePoint) -> HrxStatus {
    if stream.is_null() {
        return hrx_make_status(HrxStatusCode::InvalidArgument as i32, c"stream is NULL".as_ptr());
    }
    if position.semaphore.is_null() {
        return hrx_make_status(
            HrxStatusCode::InvalidArgument as i32,
            c"position semaphore is NULL".as_ptr(),
        );
    }
    let s = hrx_stream_flush(stream);
    if !hrx_status_is_ok(s) {
        return s;
    }
    let mut signal_value = (*stream).timepoint + 1;
    let mut wait_sem = (*position.semaphore).hal_semaphore;
    let mut wait_val = position.value;
    let mut sig_sem = (*(*stream).semaphore).hal_semaphore;
    let wait_list = ireei::iree_hal_semaphore_list_t {
        count: 1,
        semaphores: &mut wait_sem,
        payload_values: &mut wait_val,
    };
    let signal_list = ireei::iree_hal_semaphore_list_t {
        count: 1,
        semaphores: &mut sig_sem,
        payload_values: &mut signal_value,
    };
    let s = ireei::iree_hal_device_queue_barrier(
        (*(*stream).device).hal_device,
        ireei::IREE_HAL_QUEUE_AFFINITY_ANY,
        wait_list,
        signal_list,
        0,
    );
    if !iree::status_is_ok(s) {
        return hrx_status_from_iree(s);
    }
    (*stream).timepoint = signal_value;
    hrx_ok_status()
}

// --- recording ops ---

#[no_mangle]
pub unsafe extern "C" fn hrx_stream_fill_buffer(
    stream: HrxStream,
    buffer: HrxBuffer,
    offset: usize,
    size: usize,
    pattern: *const c_void,
    pattern_size: usize,
) -> HrxStatus {
    if stream.is_null() || buffer.is_null() || pattern.is_null() {
        return hrx_make_status(
            HrxStatusCode::InvalidArgument as i32,
            c"stream, buffer, or pattern is NULL".as_ptr(),
        );
    }
    let s = stream_begin_cb(stream);
    if !hrx_status_is_ok(s) {
        return s;
    }
    let target_ref = ireei::iree_hal_buffer_ref_t::make((*buffer).hal_buffer, offset as u64, size as u64);
    let s = ireei::iree_hal_command_buffer_fill_buffer(
        (*stream).pending_cb,
        target_ref,
        pattern,
        pattern_size,
        0,
    );
    if !iree::status_is_ok(s) {
        return hrx_status_from_iree(s);
    }
    let s = record_ordering_barrier(stream);
    if !iree::status_is_ok(s) {
        return hrx_status_from_iree(s);
    }
    (*stream).has_pending_work = true;
    hrx_ok_status()
}

#[no_mangle]
pub unsafe extern "C" fn hrx_stream_copy_buffer(
    stream: HrxStream,
    src: HrxBuffer,
    src_offset: usize,
    dst: HrxBuffer,
    dst_offset: usize,
    size: usize,
) -> HrxStatus {
    if stream.is_null() || src.is_null() || dst.is_null() {
        return hrx_make_status(
            HrxStatusCode::InvalidArgument as i32,
            c"stream, src, or dst is NULL".as_ptr(),
        );
    }
    let s = stream_begin_cb(stream);
    if !hrx_status_is_ok(s) {
        return s;
    }
    let source_ref = ireei::iree_hal_buffer_ref_t::make((*src).hal_buffer, src_offset as u64, size as u64);
    let target_ref = ireei::iree_hal_buffer_ref_t::make((*dst).hal_buffer, dst_offset as u64, size as u64);
    let s = ireei::iree_hal_command_buffer_copy_buffer((*stream).pending_cb, source_ref, target_ref, 0);
    if !iree::status_is_ok(s) {
        return hrx_status_from_iree(s);
    }
    let s = record_ordering_barrier(stream);
    if !iree::status_is_ok(s) {
        return hrx_status_from_iree(s);
    }
    (*stream).has_pending_work = true;
    hrx_ok_status()
}

#[no_mangle]
pub unsafe extern "C" fn hrx_stream_update_buffer(
    stream: HrxStream,
    host_data: *const c_void,
    host_data_size: usize,
    dst: HrxBuffer,
    dst_offset: usize,
) -> HrxStatus {
    if stream.is_null() || host_data.is_null() || dst.is_null() {
        return hrx_make_status(
            HrxStatusCode::InvalidArgument as i32,
            c"stream, host_data, or dst is NULL".as_ptr(),
        );
    }
    let s = stream_begin_cb(stream);
    if !hrx_status_is_ok(s) {
        return s;
    }
    let target_ref = ireei::iree_hal_buffer_ref_t::make((*dst).hal_buffer, dst_offset as u64, host_data_size as u64);
    let s = ireei::iree_hal_command_buffer_update_buffer((*stream).pending_cb, host_data, 0, target_ref, 0);
    if !iree::status_is_ok(s) {
        return hrx_status_from_iree(s);
    }
    let s = record_ordering_barrier(stream);
    if !iree::status_is_ok(s) {
        return hrx_status_from_iree(s);
    }
    (*stream).has_pending_work = true;
    hrx_ok_status()
}

#[no_mangle]
pub unsafe extern "C" fn hrx_stream_execution_barrier(stream: HrxStream) -> HrxStatus {
    if stream.is_null() {
        return hrx_make_status(HrxStatusCode::InvalidArgument as i32, c"stream is NULL".as_ptr());
    }
    let s = stream_begin_cb(stream);
    if !hrx_status_is_ok(s) {
        return s;
    }
    let barrier = ireei::iree_hal_memory_barrier_t {
        source_scope: ireei::IREE_HAL_MEMORY_ACCESS_ALL as u32,
        target_scope: ireei::IREE_HAL_MEMORY_ACCESS_ALL as u32,
    };
    let s = ireei::iree_hal_command_buffer_execution_barrier(
        (*stream).pending_cb,
        ireei::IREE_HAL_EXECUTION_STAGE_COMMAND_RETIRE,
        ireei::IREE_HAL_EXECUTION_STAGE_COMMAND_ISSUE,
        ireei::IREE_HAL_EXECUTION_BARRIER_FLAG_NONE,
        1,
        &barrier,
        0,
        core::ptr::null(),
    );
    if !iree::status_is_ok(s) {
        return hrx_status_from_iree(s);
    }
    (*stream).has_pending_work = true;
    hrx_ok_status()
}

#[no_mangle]
pub unsafe extern "C" fn hrx_stream_dispatch(
    stream: HrxStream,
    executable: HrxExecutable,
    export_ordinal: u32,
    config: *const HrxDispatchConfig,
    constants: *const c_void,
    constants_size: usize,
    bindings: *const HrxBufferRef,
    binding_count: usize,
    flags: u32,
) -> HrxStatus {
    if stream.is_null()
        || executable.is_null()
        || config.is_null()
        || (binding_count > 0 && bindings.is_null())
        || (constants_size > 0 && constants.is_null())
    {
        return hrx_make_status(HrxStatusCode::InvalidArgument as i32, c"stream, executable, config, constants, or bindings are invalid".as_ptr());
    }

    let s = stream_begin_cb(stream);
    if !hrx_status_is_ok(s) {
        return s;
    }

    let (_hold, hal_binding_list) = match build_hal_bindings(bindings, binding_count) {
        Ok(x) => x,
        Err(()) => return hrx_make_status(HrxStatusCode::InvalidArgument as i32, c"binding buffer is NULL".as_ptr()),
    };

    let hal_config = ireei::iree_hal_dispatch_config_t::new_static((*config).workgroup_size, (*config).workgroup_count);
    let hal_constants = iree::iree_const_byte_span_t { data: constants as *const u8, data_length: constants_size };
    let func = fem::iree_hal_executable_function_t { value: export_ordinal as u64 };

    let s = ireei::iree_hal_command_buffer_dispatch(
        (*stream).pending_cb,
        (*executable).hal_executable,
        func,
        hal_config,
        hal_constants,
        hal_binding_list,
        flags as u64,
    );
    if !iree::status_is_ok(s) {
        return hrx_status_from_iree(s);
    }

    let s = record_ordering_barrier(stream);
    if !iree::status_is_ok(s) {
        return hrx_status_from_iree(s);
    }

    (*stream).has_pending_work = true;
    hrx_ok_status()
}
