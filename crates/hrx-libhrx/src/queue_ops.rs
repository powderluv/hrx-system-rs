//! Rust port of libhrx/src/libhrx/queue_ops.c — direct queue operations. Each
//! call is a complete submission (wait, one op, signal).
#![allow(non_snake_case)]

use core::ffi::c_void;

use crate::buffer::{buffer_hal, HrxBuffer};
use crate::common::*;
use crate::device::HrxDevice;
use crate::executable::{executable_hal, HrxExecutable};
use crate::semaphore::HrxSemaphore;
use iree_sys as iree;
use iree_sys::fem;
use iree_sys::init as ireei;

const HRX_MAX_QUEUE_SEMAPHORES: usize = 16;

/// `hrx_semaphore_list_t` (public) = { semaphores**, values*, count }.
#[repr(C)]
pub struct HrxSemaphoreList {
    pub semaphores: *mut HrxSemaphore,
    pub values: *mut u64,
    pub count: usize,
}

/// `hrx_buffer_ref_t` (public) = { buffer, offset, length }.
#[repr(C)]
pub struct HrxBufferRef {
    pub buffer: HrxBuffer,
    pub offset: usize,
    pub length: usize,
}

/// `hrx_dispatch_config_t` (public) — NOTE field order: workgroup_count FIRST,
/// then workgroup_size, then subgroup_size (28B).
#[repr(C)]
pub struct HrxDispatchConfig {
    pub workgroup_count: [u32; 3],
    pub workgroup_size: [u32; 3],
    pub subgroup_size: u32,
}

/// `hrx_host_call_fn_t` = `hrx_status_t (*)(void *user_data)`.
pub type HrxHostCallFn = Option<unsafe extern "C" fn(*mut c_void) -> HrxStatus>;

fn normalize_affinity(affinity: u64) -> u64 {
    if affinity == 0 {
        ireei::IREE_HAL_QUEUE_AFFINITY_ANY
    } else {
        affinity
    }
}

/// Fill the caller-provided stack arrays and return an iree semaphore list view.
/// `hal`/`vals` must have >= list.count entries and outlive the returned list.
unsafe fn to_iree_sem_list(
    list: *const HrxSemaphoreList,
    hal: *mut *mut ireei::iree_hal_semaphore_t,
    vals: *mut u64,
) -> ireei::iree_hal_semaphore_list_t {
    if list.is_null() || (*list).count == 0 {
        return ireei::iree_hal_semaphore_list_t::default();
    }
    let n = (*list).count;
    for i in 0..n {
        let sem = *(*list).semaphores.add(i);
        *hal.add(i) = crate::semaphore::semaphore_hal_ptr(sem);
        *vals.add(i) = *(*list).values.add(i);
    }
    ireei::iree_hal_semaphore_list_t {
        count: n,
        semaphores: hal,
        payload_values: vals,
    }
}

#[no_mangle]
pub unsafe extern "C" fn hrx_queue_fill(
    device: HrxDevice,
    affinity: u64,
    wait_semaphores: *const HrxSemaphoreList,
    signal_semaphores: *const HrxSemaphoreList,
    buffer: HrxBuffer,
    offset: usize,
    size: usize,
    pattern: *const c_void,
    pattern_size: usize,
) -> HrxStatus {
    if device.is_null() || buffer.is_null() || pattern.is_null() {
        return hrx_make_status(
            HrxStatusCode::InvalidArgument as i32,
            c"device, buffer, or pattern is NULL".as_ptr(),
        );
    }
    let qa = normalize_affinity(affinity);
    let mut cb: *mut ireei::iree_hal_command_buffer_t = core::ptr::null_mut();
    let s = ireei::iree_hal_command_buffer_create(
        (*device).hal_device.as_ptr(),
        ireei::IREE_HAL_COMMAND_BUFFER_MODE_ONE_SHOT,
        ireei::IREE_HAL_COMMAND_CATEGORY_TRANSFER,
        qa,
        0,
        &mut cb,
    );
    if !iree::status_is_ok(s) {
        return hrx_status_from_iree(s);
    }
    let s = ireei::iree_hal_command_buffer_begin(cb);
    if !iree::status_is_ok(s) {
        ireei::iree_hal_command_buffer_release(cb);
        return hrx_status_from_iree(s);
    }
    let target_ref = ireei::iree_hal_buffer_ref_t::make(buffer_hal(buffer), offset as u64, size as u64);
    let s = ireei::iree_hal_command_buffer_fill_buffer(cb, target_ref, pattern, pattern_size, 0);
    if !iree::status_is_ok(s) {
        ireei::iree_hal_command_buffer_release(cb);
        return hrx_status_from_iree(s);
    }
    let s = ireei::iree_hal_command_buffer_end(cb);
    if !iree::status_is_ok(s) {
        ireei::iree_hal_command_buffer_release(cb);
        return hrx_status_from_iree(s);
    }
    let mut wh = [core::ptr::null_mut(); HRX_MAX_QUEUE_SEMAPHORES];
    let mut wv = [0u64; HRX_MAX_QUEUE_SEMAPHORES];
    let mut sh = [core::ptr::null_mut(); HRX_MAX_QUEUE_SEMAPHORES];
    let mut sv = [0u64; HRX_MAX_QUEUE_SEMAPHORES];
    let wl = to_iree_sem_list(wait_semaphores, wh.as_mut_ptr(), wv.as_mut_ptr());
    let sl = to_iree_sem_list(signal_semaphores, sh.as_mut_ptr(), sv.as_mut_ptr());
    let bt = ireei::iree_hal_buffer_binding_table_t::default();
    let s = ireei::iree_hal_device_queue_execute((*device).hal_device.as_ptr(), qa, wl, sl, cb, bt, 0);
    ireei::iree_hal_command_buffer_release(cb);
    hrx_status_from_iree(s)
}

#[no_mangle]
pub unsafe extern "C" fn hrx_queue_copy(
    device: HrxDevice,
    affinity: u64,
    wait_semaphores: *const HrxSemaphoreList,
    signal_semaphores: *const HrxSemaphoreList,
    src: HrxBuffer,
    src_offset: usize,
    dst: HrxBuffer,
    dst_offset: usize,
    size: usize,
) -> HrxStatus {
    if device.is_null() || src.is_null() || dst.is_null() {
        return hrx_make_status(
            HrxStatusCode::InvalidArgument as i32,
            c"device, src, or dst is NULL".as_ptr(),
        );
    }
    let qa = normalize_affinity(affinity);
    let mut cb: *mut ireei::iree_hal_command_buffer_t = core::ptr::null_mut();
    let s = ireei::iree_hal_command_buffer_create(
        (*device).hal_device.as_ptr(),
        ireei::IREE_HAL_COMMAND_BUFFER_MODE_ONE_SHOT,
        ireei::IREE_HAL_COMMAND_CATEGORY_TRANSFER,
        qa,
        0,
        &mut cb,
    );
    if !iree::status_is_ok(s) {
        return hrx_status_from_iree(s);
    }
    let s = ireei::iree_hal_command_buffer_begin(cb);
    if !iree::status_is_ok(s) {
        ireei::iree_hal_command_buffer_release(cb);
        return hrx_status_from_iree(s);
    }
    let src_ref = ireei::iree_hal_buffer_ref_t::make(buffer_hal(src), src_offset as u64, size as u64);
    let dst_ref = ireei::iree_hal_buffer_ref_t::make(buffer_hal(dst), dst_offset as u64, size as u64);
    let s = ireei::iree_hal_command_buffer_copy_buffer(cb, src_ref, dst_ref, 0);
    if !iree::status_is_ok(s) {
        ireei::iree_hal_command_buffer_release(cb);
        return hrx_status_from_iree(s);
    }
    let s = ireei::iree_hal_command_buffer_end(cb);
    if !iree::status_is_ok(s) {
        ireei::iree_hal_command_buffer_release(cb);
        return hrx_status_from_iree(s);
    }
    let mut wh = [core::ptr::null_mut(); HRX_MAX_QUEUE_SEMAPHORES];
    let mut wv = [0u64; HRX_MAX_QUEUE_SEMAPHORES];
    let mut sh = [core::ptr::null_mut(); HRX_MAX_QUEUE_SEMAPHORES];
    let mut sv = [0u64; HRX_MAX_QUEUE_SEMAPHORES];
    let wl = to_iree_sem_list(wait_semaphores, wh.as_mut_ptr(), wv.as_mut_ptr());
    let sl = to_iree_sem_list(signal_semaphores, sh.as_mut_ptr(), sv.as_mut_ptr());
    let bt = ireei::iree_hal_buffer_binding_table_t::default();
    let s = ireei::iree_hal_device_queue_execute((*device).hal_device.as_ptr(), qa, wl, sl, cb, bt, 0);
    ireei::iree_hal_command_buffer_release(cb);
    hrx_status_from_iree(s)
}

#[no_mangle]
pub unsafe extern "C" fn hrx_queue_barrier(
    device: HrxDevice,
    affinity: u64,
    wait_semaphores: *const HrxSemaphoreList,
    signal_semaphores: *const HrxSemaphoreList,
) -> HrxStatus {
    if device.is_null() {
        return hrx_make_status(HrxStatusCode::InvalidArgument as i32, c"device is NULL".as_ptr());
    }
    let mut wh = [core::ptr::null_mut(); HRX_MAX_QUEUE_SEMAPHORES];
    let mut wv = [0u64; HRX_MAX_QUEUE_SEMAPHORES];
    let mut sh = [core::ptr::null_mut(); HRX_MAX_QUEUE_SEMAPHORES];
    let mut sv = [0u64; HRX_MAX_QUEUE_SEMAPHORES];
    let wl = to_iree_sem_list(wait_semaphores, wh.as_mut_ptr(), wv.as_mut_ptr());
    let sl = to_iree_sem_list(signal_semaphores, sh.as_mut_ptr(), sv.as_mut_ptr());
    let s = ireei::iree_hal_device_queue_barrier(
        (*device).hal_device.as_ptr(),
        normalize_affinity(affinity),
        wl,
        sl,
        0,
    );
    hrx_status_from_iree(s)
}

// Host-call thunk: { callback, user_data }. The thunk is heap-allocated, passed
// as user_data to IREE, and freed inside the trampoline (matches the C).
#[repr(C)]
struct HostCallThunk {
    callback: HrxHostCallFn,
    user_data: *mut c_void,
}

unsafe extern "C" fn host_call_trampoline(
    user_data: *mut c_void,
    _args: *const u64,
    _context: *mut c_void,
) -> iree::iree_status_t {
    let thunk = user_data as *mut HostCallThunk;
    let status = ((*thunk).callback.unwrap())((*thunk).user_data);
    libc::free(thunk as *mut c_void);
    hrx_status_to_iree(status)
}

#[no_mangle]
pub unsafe extern "C" fn hrx_queue_host_call(
    device: HrxDevice,
    affinity: u64,
    wait_semaphores: *const HrxSemaphoreList,
    signal_semaphores: *const HrxSemaphoreList,
    callback: HrxHostCallFn,
    user_data: *mut c_void,
) -> HrxStatus {
    if device.is_null() || callback.is_none() {
        return hrx_make_status(
            HrxStatusCode::InvalidArgument as i32,
            c"device or callback is NULL".as_ptr(),
        );
    }
    let thunk = libc::malloc(core::mem::size_of::<HostCallThunk>()) as *mut HostCallThunk;
    if thunk.is_null() {
        return hrx_make_status(
            HrxStatusCode::OutOfMemory as i32,
            c"failed to allocate host call thunk".as_ptr(),
        );
    }
    (*thunk).callback = callback;
    (*thunk).user_data = user_data;

    let mut wh = [core::ptr::null_mut(); HRX_MAX_QUEUE_SEMAPHORES];
    let mut wv = [0u64; HRX_MAX_QUEUE_SEMAPHORES];
    let mut sh = [core::ptr::null_mut(); HRX_MAX_QUEUE_SEMAPHORES];
    let mut sv = [0u64; HRX_MAX_QUEUE_SEMAPHORES];
    let wl = to_iree_sem_list(wait_semaphores, wh.as_mut_ptr(), wv.as_mut_ptr());
    let sl = to_iree_sem_list(signal_semaphores, sh.as_mut_ptr(), sv.as_mut_ptr());

    let args: [u64; 4] = [0, 0, 0, 0];
    let call = ireei::iree_hal_host_call_t {
        fn_: Some(host_call_trampoline),
        user_data: thunk as *mut c_void,
    };
    let s = ireei::iree_hal_device_queue_host_call(
        (*device).hal_device.as_ptr(),
        normalize_affinity(affinity),
        wl,
        sl,
        call,
        args.as_ptr(),
        ireei::IREE_HAL_HOST_CALL_FLAG_NONE,
    );
    if !iree::status_is_ok(s) {
        libc::free(thunk as *mut c_void);
    }
    hrx_status_from_iree(s)
}

/// Build the iree HAL binding list from hrx bindings. Returns (vec, list); the
/// vec must outlive the list (it owns the storage `values` points at). On a NULL
/// binding buffer returns Err (caller maps to INVALID_ARGUMENT). For count==0,
/// values is NULL (matches the C calloc(0) path).
pub(crate) unsafe fn build_hal_bindings(
    bindings: *const HrxBufferRef,
    binding_count: usize,
) -> Result<(Vec<ireei::iree_hal_buffer_ref_t>, ireei::iree_hal_buffer_ref_list_t), ()> {
    if binding_count == 0 {
        return Ok((Vec::new(), ireei::iree_hal_buffer_ref_list_t { count: 0, values: core::ptr::null() }));
    }
    let mut v: Vec<ireei::iree_hal_buffer_ref_t> = Vec::with_capacity(binding_count);
    for i in 0..binding_count {
        let b = &*bindings.add(i);
        if b.buffer.is_null() {
            return Err(());
        }
        v.push(ireei::iree_hal_buffer_ref_t::make(buffer_hal(b.buffer), b.offset as u64, b.length as u64));
    }
    let list = ireei::iree_hal_buffer_ref_list_t { count: binding_count, values: v.as_ptr() };
    Ok((v, list))
}

#[no_mangle]
pub unsafe extern "C" fn hrx_queue_dispatch(
    device: HrxDevice,
    affinity: u64,
    wait_semaphores: *const HrxSemaphoreList,
    signal_semaphores: *const HrxSemaphoreList,
    executable: HrxExecutable,
    export_ordinal: u32,
    config: *const HrxDispatchConfig,
    constants: *const c_void,
    constants_size: usize,
    bindings: *const HrxBufferRef,
    binding_count: usize,
    flags: u32,
) -> HrxStatus {
    if device.is_null()
        || executable.is_null()
        || config.is_null()
        || (binding_count > 0 && bindings.is_null())
        || (constants_size > 0 && constants.is_null())
    {
        return hrx_make_status(HrxStatusCode::InvalidArgument as i32, c"device, executable, config, constants, or bindings are invalid".as_ptr());
    }

    let (_hold, hal_binding_list) = match build_hal_bindings(bindings, binding_count) {
        Ok(x) => x,
        Err(()) => return hrx_make_status(HrxStatusCode::InvalidArgument as i32, c"binding buffer is NULL".as_ptr()),
    };

    let mut wh = [core::ptr::null_mut(); HRX_MAX_QUEUE_SEMAPHORES];
    let mut wv = [0u64; HRX_MAX_QUEUE_SEMAPHORES];
    let mut sh = [core::ptr::null_mut(); HRX_MAX_QUEUE_SEMAPHORES];
    let mut sv = [0u64; HRX_MAX_QUEUE_SEMAPHORES];
    let wl = to_iree_sem_list(wait_semaphores, wh.as_mut_ptr(), wv.as_mut_ptr());
    let sl = to_iree_sem_list(signal_semaphores, sh.as_mut_ptr(), sv.as_mut_ptr());

    let hal_config = ireei::iree_hal_dispatch_config_t::new_static((*config).workgroup_size, (*config).workgroup_count);
    let hal_constants = iree::iree_const_byte_span_t { data: constants as *const u8, data_length: constants_size };
    let func = fem::iree_hal_executable_function_t { value: export_ordinal as u64 };

    let s = ireei::iree_hal_device_queue_dispatch(
        (*device).hal_device.as_ptr(),
        normalize_affinity(affinity),
        wl,
        sl,
        executable_hal(executable),
        func,
        hal_config,
        hal_constants,
        hal_binding_list,
        flags as u64,
    );
    hrx_status_from_iree(s)
}
