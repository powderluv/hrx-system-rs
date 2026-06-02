//! Rust port of libhrx/src/libhrx/executable.c — HAL executable load + export
//! introspection. The export functions go through the iree_hal_compat shim's
//! function_* API (executable.c uses the old "export" names).
//!
//! Phase-2 owned model: the opaque `hrx_executable_t` is the `Arc` data pointer of
//! an `HrxExecutableS`. retain/release are `Arc` refcount ops, and on the last
//! release the fields drop in declaration order — `hal_executable` →
//! `hal_executable_cache` → `device` — reproducing the C release sequence. The HAL
//! handles are RAII wrappers; there is no explicit `Drop`.
#![allow(non_snake_case)]

use core::ffi::{c_char, c_void};

use crate::common::*;
use crate::device::{DeviceRef, HrxDevice};
use crate::handle::{handle_ref, handle_release, handle_retain, into_handle};
use iree_hal::{HalExecutable, HalExecutableCache};
use iree_sys as iree;
use iree_sys::fem;
use iree_sys::init as ireei;

/// `hrx_executable_s` — the object behind the opaque `hrx_executable_t`.
/// Declaration order is load-bearing for drop: `hal_executable` →
/// `hal_executable_cache` → `device`, matching the C release order.
pub struct HrxExecutableS {
    hal_executable: HalExecutable,
    /// Held only for its RAII drop (releases the cache after the executable);
    /// never read directly.
    #[allow(dead_code)]
    hal_executable_cache: HalExecutableCache,
    /// RAII drop-guard holding the device reference for the executable's lifetime;
    /// never read directly.
    #[allow(dead_code)]
    device: DeviceRef,
}
pub type HrxExecutable = *mut HrxExecutableS;

/// Borrow the raw IREE executable pointer behind a handle (for dispatch in
/// queue_ops/stream).
///
/// # Safety
/// `executable` must be a live `hrx_executable_t`.
pub(crate) unsafe fn executable_hal(executable: HrxExecutable) -> *mut fem::iree_hal_executable_t {
    handle_ref(executable).hal_executable.as_ptr()
}

/// `hrx_executable_export_info_t` (public): { name, flags, constant_count,
/// binding_count, parameter_count, workgroup_size[3] } — all u32 (name is ptr).
#[repr(C)]
pub struct HrxExecutableExportInfo {
    pub name: *const c_char,
    pub flags: u32,
    pub constant_count: u32,
    pub binding_count: u32,
    pub parameter_count: u32,
    pub workgroup_size: [u32; 3],
}

unsafe fn wrap(
    device: HrxDevice,
    cache: *mut fem::iree_hal_executable_cache_t,
    exe: *mut fem::iree_hal_executable_t,
    out: *mut HrxExecutable,
) -> HrxStatus {
    let hal_executable = HalExecutable::from_owned(exe).expect("prepared executable is non-null");
    let hal_executable_cache =
        HalExecutableCache::from_owned(cache).expect("created cache is non-null");
    let device = DeviceRef::retain(device);
    *out = into_handle(HrxExecutableS { hal_executable, hal_executable_cache, device });
    hrx_ok_status()
}

#[no_mangle]
pub unsafe extern "C" fn hrx_executable_load_data(
    device: HrxDevice,
    executable_data: *const c_void,
    executable_data_size: usize,
    executable_format: *const c_char,
    executable: *mut HrxExecutable,
) -> HrxStatus {
    if device.is_null() || executable.is_null() || executable_data.is_null() || executable_data_size == 0 {
        return hrx_make_status(
            HrxStatusCode::InvalidArgument as i32,
            c"device, executable_data, or executable is invalid".as_ptr(),
        );
    }
    *executable = core::ptr::null_mut();

    let mut cache: *mut fem::iree_hal_executable_cache_t = core::ptr::null_mut();
    let s = fem::iree_hal_executable_cache_create(
        (*device).hal_device.as_ptr(),
        ireei::iree_string_view_t::cstr(c"hrx"),
        &mut cache,
    );
    if !iree::status_is_ok(s) {
        return hrx_status_from_iree(s);
    }

    // (Format inference omitted — callers pass a format; if NULL we'd need
    // iree_hal_executable_cache_infer_format. For now require a format.)
    if executable_format.is_null() || *executable_format == 0 {
        fem::iree_hal_executable_cache_release(cache);
        return hrx_make_status(
            HrxStatusCode::InvalidArgument as i32,
            c"executable_format is required".as_ptr(),
        );
    }

    let mut params = fem::iree_hal_executable_params_t::zeroed();
    fem::iree_hal_executable_params_initialize(&mut params);
    let fmt = ireei::iree_string_view_t::cstr_raw(executable_format);
    params.set_executable_format(fmt);
    params.set_executable_data(iree::iree_const_byte_span_t {
        data: executable_data as *const u8,
        data_length: executable_data_size,
    });
    params.set_caching_mode(0);

    let mut exe: *mut fem::iree_hal_executable_t = core::ptr::null_mut();
    let s = fem::iree_hal_executable_cache_prepare_executable(cache, &params, &mut exe);
    if !iree::status_is_ok(s) {
        fem::iree_hal_executable_cache_release(cache);
        return hrx_status_from_iree(s);
    }
    wrap(device, cache, exe, executable)
}

#[no_mangle]
pub unsafe extern "C" fn hrx_executable_retain(executable: HrxExecutable) {
    if executable.is_null() {
        return;
    }
    handle_retain(executable);
}

#[no_mangle]
pub unsafe extern "C" fn hrx_executable_release(executable: HrxExecutable) {
    // The HAL teardown (release executable/cache/device) moved into the field
    // drops, which run on the last reference in C order. The C code released the
    // HAL objects on every call to balance per-retain HAL retains; the owned model
    // holds one reference each and releases them once on drop — observably
    // equivalent.
    if executable.is_null() {
        return;
    }
    handle_release(executable);
}

#[no_mangle]
pub unsafe extern "C" fn hrx_executable_export_count(
    executable: HrxExecutable,
    count: *mut usize,
) -> HrxStatus {
    if executable.is_null() || count.is_null() {
        return hrx_make_status(
            HrxStatusCode::InvalidArgument as i32,
            c"executable or count is NULL".as_ptr(),
        );
    }
    *count = fem::iree_hal_executable_function_count(handle_ref(executable).hal_executable.as_ptr());
    hrx_ok_status()
}

#[no_mangle]
pub unsafe extern "C" fn hrx_executable_export_info(
    executable: HrxExecutable,
    export_ordinal: u32,
    out_info: *mut HrxExecutableExportInfo,
) -> HrxStatus {
    if executable.is_null() || out_info.is_null() {
        return hrx_make_status(
            HrxStatusCode::InvalidArgument as i32,
            c"executable or out_info is NULL".as_ptr(),
        );
    }
    let mut hal = fem::iree_hal_executable_function_info_t::zeroed();
    // iree_hal_executable_function_from_index(ordinal) — the function handle is
    // just { value: ordinal as u64 } per the index mapping.
    let func = fem::iree_hal_executable_function_t { value: export_ordinal as u64 };
    let s = fem::iree_hal_executable_function_info(handle_ref(executable).hal_executable.as_ptr(), func, &mut hal);
    if !iree::status_is_ok(s) {
        return hrx_status_from_iree(s);
    }
    (*out_info).name = hal.name.data as *const c_char;
    (*out_info).flags = hal.flags;
    (*out_info).constant_count = hal.constant_count as u32;
    (*out_info).binding_count = hal.binding_count as u32;
    (*out_info).parameter_count = hal.parameter_count as u32;
    (*out_info).workgroup_size = hal.workgroup_size;
    hrx_ok_status()
}

#[no_mangle]
pub unsafe extern "C" fn hrx_executable_lookup_export_by_name(
    executable: HrxExecutable,
    name: *const c_char,
    export_ordinal: *mut u32,
) -> HrxStatus {
    if executable.is_null() || name.is_null() || export_ordinal.is_null() {
        return hrx_make_status(
            HrxStatusCode::InvalidArgument as i32,
            c"executable, name, or export_ordinal is NULL".as_ptr(),
        );
    }
    let mut func = fem::iree_hal_executable_function_t { value: u64::MAX };
    let s = fem::iree_hal_executable_lookup_function_by_name(
        handle_ref(executable).hal_executable.as_ptr(),
        ireei::iree_string_view_t::cstr_raw(name),
        &mut func,
    );
    if !iree::status_is_ok(s) {
        return hrx_status_from_iree(s);
    }
    if func.value > u32::MAX as u64 {
        return hrx_make_status(
            HrxStatusCode::Unimplemented as i32,
            c"executable function is not a dense ordinal".as_ptr(),
        );
    }
    *export_ordinal = func.value as u32;
    hrx_ok_status()
}

/// `hrx_executable_load_file` (executable.c) — read the whole file with the
/// system host allocator, then delegate to hrx_executable_load_data. Mirrors the
/// C fopen/fseek/ftell/fread error ladder exactly so status codes match.
#[no_mangle]
pub unsafe extern "C" fn hrx_executable_load_file(
    device: HrxDevice,
    path: *const c_char,
    executable_format: *const c_char,
    executable: *mut HrxExecutable,
) -> HrxStatus {
    if device.is_null() || path.is_null() || executable.is_null() {
        return hrx_make_status(HrxStatusCode::InvalidArgument as i32, c"device, path, or executable is NULL".as_ptr());
    }
    *executable = core::ptr::null_mut();

    let file = libc::fopen(path, c"rb".as_ptr());
    if file.is_null() {
        return hrx_make_status(HrxStatusCode::NotFound as i32, c"failed to open executable file".as_ptr());
    }
    if libc::fseek(file, 0, libc::SEEK_END) != 0 {
        libc::fclose(file);
        return hrx_make_status(HrxStatusCode::Internal as i32, c"failed to seek executable file".as_ptr());
    }
    let file_size = libc::ftell(file);
    if file_size <= 0 {
        libc::fclose(file);
        return hrx_make_status(HrxStatusCode::InvalidArgument as i32, c"empty executable file".as_ptr());
    }
    if libc::fseek(file, 0, libc::SEEK_SET) != 0 {
        libc::fclose(file);
        return hrx_make_status(HrxStatusCode::Internal as i32, c"failed to rewind executable file".as_ptr());
    }

    let sys = iree::allocator_system();
    let ha = HrxHostAllocator { self_: sys.self_, ctl: sys.ctl };
    let mut file_data: *mut c_void = core::ptr::null_mut();
    let status = crate::host_allocator::hrx_host_allocator_malloc_uninitialized(ha, file_size as usize, &mut file_data);
    if !hrx_status_is_ok(status) {
        libc::fclose(file);
        return status;
    }

    let read_size = libc::fread(file_data, 1, file_size as usize, file);
    libc::fclose(file);
    if read_size != file_size as usize {
        crate::host_allocator::hrx_host_allocator_free(ha, file_data);
        return hrx_make_status(HrxStatusCode::DataLoss as i32, c"short read while loading executable file".as_ptr());
    }

    let status = hrx_executable_load_data(device, file_data, file_size as usize, executable_format, executable);
    crate::host_allocator::hrx_host_allocator_free(ha, file_data);
    status
}
