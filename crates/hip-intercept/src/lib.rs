//! Rust port of `libhrx/src/passthrough/hip_intercept.c` — the passthrough
//! library that is loaded in place of `libamdhip64.so`.
//!
//! Lifecycle (mirrors the C original):
//!  1. A high-priority constructor runs `intercept_init` at load time, and every
//!     exported function also calls `ensure_init()` (once-guarded) defensively.
//!  2. `intercept_init` dlopen's the backend from `HIP_PASSTHROUGH_BACKEND_LIB`
//!     (default `/opt/rocm/lib/libamdhip64.so`) with RTLD_NOW|RTLD_GLOBAL and
//!     fills the real function table via dlsym.
//!  3. If `HIP_INTERCEPTION_LIBRARY` is set, its `hip_interceptor_init` is given
//!     the real table and may return a wrapper table that becomes active.
//!  4. Every exported HIP symbol forwards through the ACTIVE table, so an
//!     interceptor (C or Rust) sees the calls.
//!
//! This crate currently exports the 49 table-routed functions (the set the
//! interceptor can wrap) plus the two passthrough accessors. The ~272
//! direct-passthrough symbols from the C file (dlsym + log, bypassing the table)
//! are added separately so the .so becomes a complete drop-in.
#![allow(non_snake_case)]

use core::ffi::{c_char, c_float, c_int, c_uint, c_void};
use core::ptr;
use core::sync::atomic::{AtomicPtr, Ordering};
use std::sync::Once;

use hip_function_table::*;
use libc::{dlopen, dlsym, RTLD_GLOBAL, RTLD_LOCAL, RTLD_NOW};

// ---------------------------------------------------------------------------
// Global state (mirrors the C statics).
// ---------------------------------------------------------------------------
static INIT: Once = Once::new();
static G_BACKEND_LIB: AtomicPtr<c_void> = AtomicPtr::new(ptr::null_mut());
static G_INTERCEPTOR_LIB: AtomicPtr<c_void> = AtomicPtr::new(ptr::null_mut());
static G_ACTIVE_TABLE: AtomicPtr<HipFunctionTable> = AtomicPtr::new(ptr::null_mut());
static G_INTERCEPTOR_SHUTDOWN: AtomicPtr<c_void> = AtomicPtr::new(ptr::null_mut());

/// The real table, filled by dlsym. Boxed and leaked so its address is stable
/// (the interceptor is handed `&mut` to it, exactly like the C `&g_real_table`).
static G_REAL_TABLE: AtomicPtr<HipFunctionTable> = AtomicPtr::new(ptr::null_mut());

#[inline]
fn active() -> *mut HipFunctionTable {
    let t = G_ACTIVE_TABLE.load(Ordering::Acquire);
    if t.is_null() {
        ensure_init();
        G_ACTIVE_TABLE.load(Ordering::Acquire)
    } else {
        t
    }
}

unsafe fn load_sym(lib: *mut c_void, name: &[u8]) -> *mut c_void {
    // name must be NUL-terminated.
    dlsym(lib, name.as_ptr() as *const c_char)
}

/// Resolve every table slot from the backend via dlsym. The macro maps each
/// field to its C symbol name and transmutes the resolved pointer to the
/// field's Option<fn> type.
macro_rules! load_all {
    ($lib:expr, $t:expr, [ $($field:ident),* $(,)? ]) => {{
        $(
            let s = load_sym($lib, concat!(stringify!($field), "\0").as_bytes());
            $t.$field = core::mem::transmute::<*mut c_void, _>(s);
        )*
    }};
}

fn intercept_init() {
    unsafe {
        // 1. Backend.
        let backend_path = std::env::var("HIP_PASSTHROUGH_BACKEND_LIB")
            .ok()
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "/opt/rocm/lib/libamdhip64.so".to_string());
        let cpath = std::ffi::CString::new(backend_path.clone()).unwrap();
        let backend = dlopen(cpath.as_ptr(), RTLD_NOW | RTLD_GLOBAL);
        if backend.is_null() {
            let err = libc::dlerror();
            let msg = if err.is_null() {
                "unknown".to_string()
            } else {
                std::ffi::CStr::from_ptr(err).to_string_lossy().into_owned()
            };
            eprintln!("hip_intercept: failed to load backend: {msg}");
            std::process::abort();
        }
        G_BACKEND_LIB.store(backend, Ordering::Release);

        // 2. Fill the real table.
        let mut real: HipFunctionTable = core::mem::zeroed();
        load_all!(backend, real, [
            hipInit, hipDriverGetVersion, hipRuntimeGetVersion, hipGetDevice,
            hipGetDeviceCount, hipSetDevice, hipDeviceReset, hipDeviceSynchronize,
            hipGetDeviceProperties, hipDeviceGetAttribute, hipDeviceGetName,
            hipMalloc, hipFree, hipHostMalloc, hipHostFree, hipMemGetInfo,
            hipMemcpy, hipMemcpyAsync, hipMemset, hipMemsetAsync,
            hipStreamCreate, hipStreamCreateWithFlags, hipStreamDestroy,
            hipStreamSynchronize, hipStreamQuery, hipStreamWaitEvent,
            hipEventCreate, hipEventCreateWithFlags, hipEventDestroy,
            hipEventRecord, hipEventSynchronize, hipEventQuery, hipEventElapsedTime,
            hipModuleLoad, hipModuleLoadData, hipModuleUnload, hipModuleGetFunction,
            hipModuleGetGlobal, hipModuleLaunchKernel, hipLaunchKernel,
            hipExtModuleLaunchKernel, __hipRegisterFatBinary,
            __hipUnregisterFatBinary, __hipRegisterFunction, __hipRegisterVar,
            hipGetErrorString, hipGetErrorName, hipGetLastError, hipPeekAtLastError,
        ]);
        real.version = 1;
        real.struct_size = core::mem::size_of::<HipFunctionTable>() as u32;

        let real_ptr = Box::into_raw(Box::new(real));
        G_REAL_TABLE.store(real_ptr, Ordering::Release);
        G_ACTIVE_TABLE.store(real_ptr, Ordering::Release);

        // 3. Optional interceptor.
        if let Ok(path) = std::env::var("HIP_INTERCEPTION_LIBRARY") {
            if !path.is_empty() {
                let cp = std::ffi::CString::new(path).unwrap();
                let ilib = dlopen(cp.as_ptr(), RTLD_NOW | RTLD_LOCAL);
                if !ilib.is_null() {
                    G_INTERCEPTOR_LIB.store(ilib, Ordering::Release);
                    let init = load_sym(ilib, b"hip_interceptor_init\0");
                    if !init.is_null() {
                        let init_fn: pfn_hip_interceptor_init =
                            core::mem::transmute::<*mut c_void, _>(init);
                        if let Some(f) = init_fn {
                            let table = f(real_ptr);
                            if !table.is_null() {
                                G_ACTIVE_TABLE.store(table, Ordering::Release);
                            }
                        }
                    }
                    let sh = load_sym(ilib, b"hip_interceptor_shutdown\0");
                    G_INTERCEPTOR_SHUTDOWN.store(sh, Ordering::Release);
                }
            }
        }
    }
}

fn ensure_init() {
    INIT.call_once(intercept_init);
}

/// `constructor(101)` equivalent: run init as early as possible at load.
#[ctor::ctor]
fn early_init() {
    ensure_init();
}

/// Destructor: call the interceptor's shutdown and dlclose it.
#[ctor::dtor]
fn intercept_fini() {
    unsafe {
        let sh = G_INTERCEPTOR_SHUTDOWN.load(Ordering::Acquire);
        if !sh.is_null() {
            let f: pfn_hip_interceptor_shutdown =
                core::mem::transmute::<*mut c_void, _>(sh);
            if let Some(f) = f {
                f();
            }
        }
        let ilib = G_INTERCEPTOR_LIB.load(Ordering::Acquire);
        if !ilib.is_null() {
            libc::dlclose(ilib);
        }
    }
}

// ---------------------------------------------------------------------------
// Passthrough accessors.
// ---------------------------------------------------------------------------
#[no_mangle]
pub extern "C" fn hip_passthrough_get_real_table() -> *mut HipFunctionTable {
    ensure_init();
    G_REAL_TABLE.load(Ordering::Acquire)
}

#[no_mangle]
pub extern "C" fn hip_passthrough_get_active_table() -> *mut HipFunctionTable {
    ensure_init();
    active()
}

// ---------------------------------------------------------------------------
// Table-routed exports.
//
// Each forwards through the ACTIVE table slot, returning the C original's
// "missing slot" default: 0 for hipError_t (the C FWD macros cast 0), NULL for
// pointer returns, "unknown" for the two error-string getters, and nothing for
// void. The `fwd!` macro covers the hipError_t-returning majority.
// ---------------------------------------------------------------------------
macro_rules! fwd {
    // ret hipError_t, N args
    ($name:ident ( $($an:ident : $at:ty),* $(,)? )) => {
        #[no_mangle]
        pub unsafe extern "C" fn $name($($an: $at),*) -> hipError_t {
            ensure_init();
            match (*active()).$name {
                Some(f) => f($($an),*),
                None => 0,
            }
        }
    };
}

// Device management
fwd!(hipInit(flags: c_uint));
fwd!(hipDriverGetVersion(driver_version: *mut c_int));
fwd!(hipRuntimeGetVersion(runtime_version: *mut c_int));
fwd!(hipGetDevice(device_id: *mut c_int));
fwd!(hipGetDeviceCount(count: *mut c_int));
fwd!(hipSetDevice(device_id: c_int));
fwd!(hipDeviceReset());
fwd!(hipDeviceSynchronize());
fwd!(hipGetDeviceProperties(prop: *mut hipDeviceProp_t, device_id: c_int));
fwd!(hipDeviceGetAttribute(value: *mut c_int, attr: hipDeviceAttribute_t, device_id: c_int));
fwd!(hipDeviceGetName(name: *mut c_char, len: c_int, device_id: c_int));

// Memory management
fwd!(hipMalloc(p: *mut *mut c_void, size: usize));
fwd!(hipFree(p: *mut c_void));
fwd!(hipHostMalloc(p: *mut *mut c_void, size: usize, flags: c_uint));
fwd!(hipHostFree(p: *mut c_void));
fwd!(hipMemGetInfo(free_: *mut usize, total: *mut usize));

// Memory copy
fwd!(hipMemcpy(dst: *mut c_void, src: *const c_void, n: usize, kind: hipMemcpyKind));
fwd!(hipMemcpyAsync(dst: *mut c_void, src: *const c_void, n: usize, kind: hipMemcpyKind, stream: hipStream_t));
fwd!(hipMemset(dst: *mut c_void, value: c_int, n: usize));
fwd!(hipMemsetAsync(dst: *mut c_void, value: c_int, n: usize, stream: hipStream_t));

// Stream management
fwd!(hipStreamCreate(stream: *mut hipStream_t));
fwd!(hipStreamCreateWithFlags(stream: *mut hipStream_t, flags: c_uint));
fwd!(hipStreamDestroy(stream: hipStream_t));
fwd!(hipStreamSynchronize(stream: hipStream_t));
fwd!(hipStreamQuery(stream: hipStream_t));
fwd!(hipStreamWaitEvent(stream: hipStream_t, event: hipEvent_t, flags: c_uint));

// Event management
fwd!(hipEventCreate(event: *mut hipEvent_t));
fwd!(hipEventCreateWithFlags(event: *mut hipEvent_t, flags: c_uint));
fwd!(hipEventDestroy(event: hipEvent_t));
fwd!(hipEventRecord(event: hipEvent_t, stream: hipStream_t));
fwd!(hipEventSynchronize(event: hipEvent_t));
fwd!(hipEventQuery(event: hipEvent_t));
fwd!(hipEventElapsedTime(ms: *mut c_float, start: hipEvent_t, stop: hipEvent_t));

// Module management
fwd!(hipModuleLoad(module: *mut hipModule_t, fname: *const c_char));
fwd!(hipModuleLoadData(module: *mut hipModule_t, image: *const c_void));
fwd!(hipModuleUnload(module: hipModule_t));
fwd!(hipModuleGetFunction(function: *mut hipFunction_t, module: hipModule_t, kname: *const c_char));
fwd!(hipModuleGetGlobal(dptr: *mut hipDeviceptr_t, bytes: *mut usize, hmod: hipModule_t, name: *const c_char));

// Error handling (hipError_t-returning)
fwd!(hipGetLastError());
fwd!(hipPeekAtLastError());

// --- Non-hipError_t table-routed exports (manual) ---

#[no_mangle]
pub unsafe extern "C" fn hipModuleLaunchKernel(
    f: hipFunction_t,
    gx: c_uint,
    gy: c_uint,
    gz: c_uint,
    bx: c_uint,
    by: c_uint,
    bz: c_uint,
    shared: c_uint,
    stream: hipStream_t,
    kernel_params: *mut *mut c_void,
    extra: *mut *mut c_void,
) -> hipError_t {
    ensure_init();
    match (*active()).hipModuleLaunchKernel {
        Some(g) => g(f, gx, gy, gz, bx, by, bz, shared, stream, kernel_params, extra),
        None => 0,
    }
}

#[no_mangle]
pub unsafe extern "C" fn hipLaunchKernel(
    function_address: *const c_void,
    num_blocks: dim3,
    dim_blocks: dim3,
    args: *mut *mut c_void,
    shared: usize,
    stream: hipStream_t,
) -> hipError_t {
    ensure_init();
    match (*active()).hipLaunchKernel {
        Some(g) => g(function_address, num_blocks, dim_blocks, args, shared, stream),
        None => 0,
    }
}

#[no_mangle]
pub unsafe extern "C" fn hipExtModuleLaunchKernel(
    f: hipFunction_t,
    gx: c_uint,
    gy: c_uint,
    gz: c_uint,
    lx: c_uint,
    ly: c_uint,
    lz: c_uint,
    shared: usize,
    stream: hipStream_t,
    kernel_params: *mut *mut c_void,
    extra: *mut *mut c_void,
    start_event: hipEvent_t,
    stop_event: hipEvent_t,
    flags: c_uint,
) -> hipError_t {
    ensure_init();
    match (*active()).hipExtModuleLaunchKernel {
        Some(g) => g(f, gx, gy, gz, lx, ly, lz, shared, stream, kernel_params, extra, start_event, stop_event, flags),
        None => 0,
    }
}

#[no_mangle]
pub unsafe extern "C" fn __hipRegisterFatBinary(data: *const c_void) -> *mut *mut c_void {
    ensure_init();
    match (*active()).__hipRegisterFatBinary {
        Some(g) => g(data),
        None => ptr::null_mut(),
    }
}

#[no_mangle]
pub unsafe extern "C" fn __hipUnregisterFatBinary(handle: *mut *mut c_void) {
    ensure_init();
    if let Some(g) = (*active()).__hipUnregisterFatBinary {
        g(handle);
    }
}

#[no_mangle]
pub unsafe extern "C" fn __hipRegisterFunction(
    handle: *mut *mut c_void,
    host_fun: *const c_char,
    device_fun: *mut c_char,
    device_name: *const c_char,
    thread_limit: c_int,
    tid: *mut c_void,
    bid: *mut c_void,
    block_dim: *mut dim3,
    grid_dim: *mut dim3,
    w_size: *mut c_int,
) {
    ensure_init();
    if let Some(g) = (*active()).__hipRegisterFunction {
        g(handle, host_fun, device_fun, device_name, thread_limit, tid, bid, block_dim, grid_dim, w_size);
    }
}

#[no_mangle]
pub unsafe extern "C" fn __hipRegisterVar(
    handle: *mut *mut c_void,
    host_var: *mut c_char,
    device_address: *mut c_char,
    device_name: *const c_char,
    ext: c_int,
    size: usize,
    constant: c_int,
    global: c_int,
) {
    ensure_init();
    if let Some(g) = (*active()).__hipRegisterVar {
        g(handle, host_var, device_address, device_name, ext, size, constant, global);
    }
}

#[no_mangle]
pub unsafe extern "C" fn hipGetErrorString(error: hipError_t) -> *const c_char {
    ensure_init();
    match (*active()).hipGetErrorString {
        Some(g) => g(error),
        None => c"unknown".as_ptr(),
    }
}

#[no_mangle]
pub unsafe extern "C" fn hipGetErrorName(error: hipError_t) -> *const c_char {
    ensure_init();
    match (*active()).hipGetErrorName {
        Some(g) => g(error),
        None => c"unknown".as_ptr(),
    }
}
