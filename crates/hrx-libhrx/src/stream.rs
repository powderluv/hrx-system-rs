//! Rust port of libhrx/src/libhrx/stream.c — streams own a timeline semaphore +
//! a pending one-shot command buffer; ops accumulate and flush on demand.
//!
//! Phase-2 owned model: the opaque `hrx_stream_t` is the `Arc` data pointer of an
//! `HrxStreamS`. retain/release are `Arc` refcount ops; the stream holds one
//! device reference (`DeviceRef`) and owns its timeline semaphore
//! (`SemaphoreGuard`) for its lifetime. The mutable state — `timepoint`,
//! `pending_cb`, `has_pending_work` — lives in `Cell`s (single-threaded, matching
//! the C non-atomic `*mut`-alias writes; not made atomic). An explicit `Drop`
//! reproduces the exact last-ref teardown: flush pending work → wait on the
//! timeline → release a leftover command buffer → (struct freed) → release the
//! semaphore → release the device.
#![allow(non_snake_case)]

use core::cell::Cell;
use core::ffi::c_void;

use crate::buffer::HrxBuffer;
use crate::common::*;
use crate::device::{DeviceRef, HrxDevice};
use crate::executable::HrxExecutable;
use crate::handle::{handle_ref, handle_release, handle_retain, into_handle};
use crate::queue_ops::{build_hal_bindings, HrxBufferRef, HrxDispatchConfig};
use crate::semaphore::{
    hrx_semaphore_create, hrx_semaphore_query, hrx_semaphore_wait, semaphore_hal_ptr,
    HrxSemaphore, SemaphoreGuard,
};
use iree_sys as iree;
use iree_sys::fem;
use iree_sys::init as ireei;

/// Internal object behind the opaque `hrx_stream_t`. Field declaration order is
/// load-bearing for `Drop`: the `Cell`s/flags drop as no-ops, then `semaphore`
/// (released), then `device` (released) — the C release order.
pub struct HrxStreamS {
    timepoint: Cell<u64>,
    pending_cb: Cell<*mut ireei::iree_hal_command_buffer_t>,
    has_pending_work: Cell<bool>,
    #[allow(dead_code)]
    flags: u32,
    semaphore: SemaphoreGuard,
    device: DeviceRef,
}

pub type HrxStream = *mut HrxStreamS;

/// `hrx_timeline_point_t` (public) = { hrx_semaphore_t semaphore; uint64_t value }.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct HrxTimelinePoint {
    pub semaphore: HrxSemaphore,
    pub value: u64,
}

impl HrxStreamS {
    /// Lazily create + begin the one-shot command buffer (idempotent; at most one
    /// outstanding).
    unsafe fn begin_cb(&self) -> HrxStatus {
        if !self.pending_cb.get().is_null() {
            return hrx_ok_status();
        }
        let mut cb: *mut ireei::iree_hal_command_buffer_t = core::ptr::null_mut();
        let s = ireei::iree_hal_command_buffer_create(
            (*self.device.as_ptr()).hal_device.as_ptr(),
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
            self.pending_cb.set(core::ptr::null_mut());
            return hrx_status_from_iree(s);
        }
        self.pending_cb.set(cb);
        hrx_ok_status()
    }

    unsafe fn record_ordering_barrier(&self) -> iree::iree_status_t {
        let barrier = ireei::iree_hal_memory_barrier_t {
            source_scope: ireei::IREE_HAL_MEMORY_ACCESS_ALL as u32,
            target_scope: ireei::IREE_HAL_MEMORY_ACCESS_ALL as u32,
        };
        ireei::iree_hal_command_buffer_execution_barrier(
            self.pending_cb.get(),
            ireei::IREE_HAL_EXECUTION_STAGE_COMMAND_RETIRE,
            ireei::IREE_HAL_EXECUTION_STAGE_COMMAND_ISSUE,
            ireei::IREE_HAL_EXECUTION_BARRIER_FLAG_NONE,
            1,
            &barrier,
            0,
            core::ptr::null(),
        )
    }

    /// End + submit the pending command buffer, advancing the timeline. On submit
    /// failure the CB is left installed and `has_pending_work` stays true (no
    /// rollback) — reclaimed later by the release path, matching C.
    unsafe fn flush_inner(&self) -> HrxStatus {
        if !self.has_pending_work.get() || self.pending_cb.get().is_null() {
            return hrx_ok_status();
        }
        let cb = self.pending_cb.get();
        let s = ireei::iree_hal_command_buffer_end(cb);
        if !iree::status_is_ok(s) {
            return hrx_status_from_iree(s);
        }
        let tp = self.timepoint.get();
        let mut wait_value = tp;
        let mut signal_value = tp + 1;
        let mut sem = semaphore_hal_ptr(self.semaphore.as_handle());
        let wait_list = ireei::iree_hal_semaphore_list_t {
            count: if tp > 0 { 1 } else { 0 },
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
            (*self.device.as_ptr()).hal_device.as_ptr(),
            ireei::IREE_HAL_QUEUE_AFFINITY_ANY,
            wait_list,
            signal_list,
            cb,
            binding_table,
            0,
        );
        if !iree::status_is_ok(s) {
            return hrx_status_from_iree(s);
        }
        self.timepoint.set(signal_value);
        ireei::iree_hal_command_buffer_release(cb);
        self.pending_cb.set(core::ptr::null_mut());
        self.has_pending_work.set(false);
        hrx_ok_status()
    }
}

impl Drop for HrxStreamS {
    fn drop(&mut self) {
        // Last-ref teardown (the explicit C release ordering); the `semaphore` and
        // `device` fields drop after this, releasing their one reference each.
        // SAFETY: drop runs only when the last reference is gone.
        unsafe {
            if self.has_pending_work.get() {
                hrx_status_drop(self.flush_inner());
            }
            if self.timepoint.get() > 0 {
                hrx_status_drop(hrx_semaphore_wait(
                    self.semaphore.as_handle(),
                    self.timepoint.get(),
                    u64::MAX,
                ));
            }
            let cb = self.pending_cb.get();
            if !cb.is_null() {
                ireei::iree_hal_command_buffer_release(cb);
            }
        }
    }
}

// --- crate-visible accessors (buffer.rs's hrx_buffer_allocate drives a stream) ---

/// # Safety: `stream` must be a live `hrx_stream_t`.
pub(crate) unsafe fn stream_device(stream: HrxStream) -> HrxDevice {
    handle_ref(stream).device.as_ptr()
}
/// # Safety: `stream` must be a live `hrx_stream_t`.
pub(crate) unsafe fn stream_semaphore(stream: HrxStream) -> HrxSemaphore {
    handle_ref(stream).semaphore.as_handle()
}
/// # Safety: `stream` must be a live `hrx_stream_t`.
pub(crate) unsafe fn stream_timepoint(stream: HrxStream) -> u64 {
    handle_ref(stream).timepoint.get()
}
/// # Safety: `stream` must be a live `hrx_stream_t`.
pub(crate) unsafe fn stream_set_timepoint(stream: HrxStream, value: u64) {
    handle_ref(stream).timepoint.set(value);
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
    // Retain the device first; if semaphore creation fails, `device_ref` drops on
    // the early return and releases it (the C code leaks the device on this path —
    // a pre-existing bug; we fix it, which is unobservable since creation succeeds
    // in practice).
    let device_ref = DeviceRef::retain(device);
    let mut sem: HrxSemaphore = core::ptr::null_mut();
    let st = hrx_semaphore_create(device, 0, &mut sem);
    if !hrx_status_is_ok(st) {
        return st;
    }
    *stream = into_handle(HrxStreamS {
        timepoint: Cell::new(0),
        pending_cb: Cell::new(core::ptr::null_mut()),
        has_pending_work: Cell::new(false),
        flags,
        semaphore: SemaphoreGuard::from_born(sem),
        device: device_ref,
    });
    hrx_ok_status()
}

#[no_mangle]
pub unsafe extern "C" fn hrx_stream_retain(stream: HrxStream) {
    handle_retain(stream);
}

#[no_mangle]
pub unsafe extern "C" fn hrx_stream_release(stream: HrxStream) {
    handle_release(stream);
}

#[no_mangle]
pub unsafe extern "C" fn hrx_stream_flush(stream: HrxStream) -> HrxStatus {
    if stream.is_null() {
        return hrx_make_status(HrxStatusCode::InvalidArgument as i32, c"stream is NULL".as_ptr());
    }
    handle_ref(stream).flush_inner()
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
    let st = handle_ref(stream);
    if st.timepoint.get() == 0 {
        return hrx_ok_status();
    }
    hrx_semaphore_wait(st.semaphore.as_handle(), st.timepoint.get(), u64::MAX)
}

#[no_mangle]
pub unsafe extern "C" fn hrx_stream_query(stream: HrxStream, complete: *mut bool) -> HrxStatus {
    if stream.is_null() || complete.is_null() {
        return hrx_make_status(
            HrxStatusCode::InvalidArgument as i32,
            c"stream or complete is NULL".as_ptr(),
        );
    }
    let st = handle_ref(stream);
    if st.timepoint.get() == 0 {
        *complete = true;
        return hrx_ok_status();
    }
    let mut current: u64 = 0;
    let s = hrx_semaphore_query(st.semaphore.as_handle(), &mut current);
    if !hrx_status_is_ok(s) {
        return s;
    }
    *complete = current >= st.timepoint.get();
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
    *semaphore = handle_ref(stream).semaphore.as_handle();
    hrx_ok_status()
}

#[no_mangle]
pub unsafe extern "C" fn hrx_stream_get_device(
    stream: HrxStream,
    device: *mut HrxDevice,
) -> HrxStatus {
    if stream.is_null() || device.is_null() {
        return hrx_make_status(
            HrxStatusCode::InvalidArgument as i32,
            c"stream or device is NULL".as_ptr(),
        );
    }
    *device = handle_ref(stream).device.as_ptr();
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
    let st = handle_ref(stream);
    (*position).semaphore = st.semaphore.as_handle();
    (*position).value = st.timepoint.get();
    hrx_ok_status()
}

#[no_mangle]
pub unsafe extern "C" fn hrx_stream_advance_timeline(
    stream: HrxStream,
    value: *mut u64,
) -> HrxStatus {
    if stream.is_null() || value.is_null() {
        return hrx_make_status(
            HrxStatusCode::InvalidArgument as i32,
            c"stream or value is NULL".as_ptr(),
        );
    }
    let st = handle_ref(stream);
    st.timepoint.set(st.timepoint.get() + 1);
    *value = st.timepoint.get();
    hrx_ok_status()
}

#[no_mangle]
pub unsafe extern "C" fn hrx_stream_wait_on(
    stream: HrxStream,
    position: HrxTimelinePoint,
) -> HrxStatus {
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
    let st = handle_ref(stream);
    let mut signal_value = st.timepoint.get() + 1;
    let mut wait_sem = semaphore_hal_ptr(position.semaphore);
    let mut wait_val = position.value;
    let mut sig_sem = semaphore_hal_ptr(st.semaphore.as_handle());
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
        (*st.device.as_ptr()).hal_device.as_ptr(),
        ireei::IREE_HAL_QUEUE_AFFINITY_ANY,
        wait_list,
        signal_list,
        0,
    );
    if !iree::status_is_ok(s) {
        return hrx_status_from_iree(s);
    }
    st.timepoint.set(signal_value);
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
    let st = handle_ref(stream);
    let s = st.begin_cb();
    if !hrx_status_is_ok(s) {
        return s;
    }
    let target_ref =
        ireei::iree_hal_buffer_ref_t::make((*buffer).hal_buffer, offset as u64, size as u64);
    let s = ireei::iree_hal_command_buffer_fill_buffer(
        st.pending_cb.get(),
        target_ref,
        pattern,
        pattern_size,
        0,
    );
    if !iree::status_is_ok(s) {
        return hrx_status_from_iree(s);
    }
    let s = st.record_ordering_barrier();
    if !iree::status_is_ok(s) {
        return hrx_status_from_iree(s);
    }
    st.has_pending_work.set(true);
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
    let st = handle_ref(stream);
    let s = st.begin_cb();
    if !hrx_status_is_ok(s) {
        return s;
    }
    let source_ref =
        ireei::iree_hal_buffer_ref_t::make((*src).hal_buffer, src_offset as u64, size as u64);
    let target_ref =
        ireei::iree_hal_buffer_ref_t::make((*dst).hal_buffer, dst_offset as u64, size as u64);
    let s = ireei::iree_hal_command_buffer_copy_buffer(st.pending_cb.get(), source_ref, target_ref, 0);
    if !iree::status_is_ok(s) {
        return hrx_status_from_iree(s);
    }
    let s = st.record_ordering_barrier();
    if !iree::status_is_ok(s) {
        return hrx_status_from_iree(s);
    }
    st.has_pending_work.set(true);
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
    let st = handle_ref(stream);
    let s = st.begin_cb();
    if !hrx_status_is_ok(s) {
        return s;
    }
    let target_ref =
        ireei::iree_hal_buffer_ref_t::make((*dst).hal_buffer, dst_offset as u64, host_data_size as u64);
    let s = ireei::iree_hal_command_buffer_update_buffer(st.pending_cb.get(), host_data, 0, target_ref, 0);
    if !iree::status_is_ok(s) {
        return hrx_status_from_iree(s);
    }
    let s = st.record_ordering_barrier();
    if !iree::status_is_ok(s) {
        return hrx_status_from_iree(s);
    }
    st.has_pending_work.set(true);
    hrx_ok_status()
}

#[no_mangle]
pub unsafe extern "C" fn hrx_stream_execution_barrier(stream: HrxStream) -> HrxStatus {
    if stream.is_null() {
        return hrx_make_status(HrxStatusCode::InvalidArgument as i32, c"stream is NULL".as_ptr());
    }
    let st = handle_ref(stream);
    let s = st.begin_cb();
    if !hrx_status_is_ok(s) {
        return s;
    }
    let barrier = ireei::iree_hal_memory_barrier_t {
        source_scope: ireei::IREE_HAL_MEMORY_ACCESS_ALL as u32,
        target_scope: ireei::IREE_HAL_MEMORY_ACCESS_ALL as u32,
    };
    let s = ireei::iree_hal_command_buffer_execution_barrier(
        st.pending_cb.get(),
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
    st.has_pending_work.set(true);
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

    let st = handle_ref(stream);
    let s = st.begin_cb();
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
        st.pending_cb.get(),
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

    let s = st.record_ordering_barrier();
    if !iree::status_is_ok(s) {
        return hrx_status_from_iree(s);
    }

    st.has_pending_work.set(true);
    hrx_ok_status()
}
