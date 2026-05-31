//! Rust port of `libhrx/src/passthrough/hip_logging.c`.
//!
//! An interceptor `.so` loaded by `libhip_intercept.so`. It receives the real
//! HIP function table, returns a wrapper table whose entries log each call (in
//! the exact textual format of the C interceptor) and forward to the real
//! function. Verified against the C interceptor by the differential harness.
#![allow(non_snake_case)]

use core::ffi::{c_char, c_float, c_int, c_uint, c_void};
use core::sync::atomic::{AtomicPtr, Ordering::Relaxed};
use std::ffi::CStr;
use std::sync::Mutex;

use hip_function_table::*;
use libc::{c_void as lc_void, size_t, FILE};

// ---------------------------------------------------------------------------
// Global state (mirrors g_real / g_wrapper / g_log_file / g_log_level).
// ---------------------------------------------------------------------------
static G_REAL: AtomicPtr<HipFunctionTable> = AtomicPtr::new(core::ptr::null_mut());

struct Logger {
    file: *mut FILE,
    level: c_int,
    owns_file: bool,
}
unsafe impl Send for Logger {}
static LOG: Mutex<Logger> = Mutex::new(Logger {
    file: core::ptr::null_mut(),
    level: 2,
    owns_file: false,
});

#[inline]
fn real() -> &'static HipFunctionTable {
    // Safe: set once in hip_interceptor_init before any wrapper runs.
    unsafe { &*G_REAL.load(Relaxed) }
}

/// Mirror of `log_msg`: timestamp prefix + message + newline, level-gated.
fn log_msg(level: c_int, msg: &str) {
    let g = LOG.lock().unwrap();
    if level > g.level || g.file.is_null() {
        return;
    }
    let mut ts = libc::timespec { tv_sec: 0, tv_nsec: 0 };
    unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut ts) };
    let line = format!("[{}.{:06}] {}\n", ts.tv_sec, ts.tv_nsec / 1000, msg);
    unsafe {
        libc::fwrite(line.as_ptr() as *const lc_void, 1, line.len() as size_t, g.file);
        libc::fflush(g.file);
    }
}

/// glibc `%p` semantics: NULL -> "(nil)", else "0x<hex>".
fn p<T>(ptr: *const T) -> String {
    if ptr.is_null() {
        "(nil)".to_string()
    } else {
        format!("0x{:x}", ptr as usize)
    }
}

/// glibc `%s` after the C code's `x ? x : "(null)"` guard.
unsafe fn cstr(ptr: *const c_char) -> String {
    if ptr.is_null() {
        "(null)".to_string()
    } else {
        CStr::from_ptr(ptr).to_string_lossy().into_owned()
    }
}

/// Mirror of `device_attribute_name`.
fn device_attribute_name(attr: hipDeviceAttribute_t) -> String {
    let s = match attr {
        0 => "MaxThreadsPerBlock",
        1 => "MaxBlockDimX",
        2 => "MaxBlockDimY",
        3 => "MaxBlockDimZ",
        4 => "MaxGridDimX",
        5 => "MaxGridDimY",
        6 => "MaxGridDimZ",
        7 => "MaxSharedMemoryPerBlock",
        8 => "TotalConstantMemory",
        9 => "WarpSize",
        10 => "MaxRegistersPerBlock",
        11 => "ClockRate",
        15 => "MemoryClockRate",
        16 => "MemoryBusWidth",
        17 => "MultiProcessorCount",
        19 => "ComputeCapabilityMajor",
        20 => "ComputeCapabilityMinor",
        21 => "L2CacheSize",
        22 => "MaxThreadsPerMultiProcessor",
        24 => "ConcurrentKernels",
        28 => "PCIBusId",
        29 => "PCIDeviceId",
        32 => "MaxSharedMemoryPerMultiprocessor",
        43 => "IsMultiGpuBoard",
        70 => "CooperativeLaunch",
        71 => "CooperativeMultiDeviceLaunch",
        87 => "PageableMemoryAccess",
        95 => "ManagedMemory",
        96 => "DirectManagedMemAccessFromHost",
        100 => "ConcurrentManagedAccess",
        101 => "PageableMemoryAccessUsesHostPageTables",
        1000 => "GCN_ARCH",
        1003 => "GCN_ARCH_NAME",
        _ => return format!("attr_{}", attr),
    };
    s.to_string()
}

// ---------------------------------------------------------------------------
// Wrapper functions (one per overridden table entry).
// ---------------------------------------------------------------------------
extern "C" fn wrap_hipInit(flags: c_uint) -> hipError_t {
    let err = unsafe { (real().hipInit.unwrap())(flags) };
    log_msg(2, &format!("hipInit(flags=0x{:x}) -> {}", flags, err));
    err
}

extern "C" fn wrap_hipDriverGetVersion(v: *mut c_int) -> hipError_t {
    let err = unsafe { (real().hipDriverGetVersion.unwrap())(v) };
    let val = if v.is_null() { -1 } else { unsafe { *v } };
    log_msg(2, &format!("hipDriverGetVersion() -> version={}, ret={}", val, err));
    err
}

extern "C" fn wrap_hipRuntimeGetVersion(v: *mut c_int) -> hipError_t {
    let err = unsafe { (real().hipRuntimeGetVersion.unwrap())(v) };
    let val = if v.is_null() { -1 } else { unsafe { *v } };
    log_msg(2, &format!("hipRuntimeGetVersion() -> version={}, ret={}", val, err));
    err
}

extern "C" fn wrap_hipGetDevice(id: *mut c_int) -> hipError_t {
    let err = unsafe { (real().hipGetDevice.unwrap())(id) };
    let val = if id.is_null() { -1 } else { unsafe { *id } };
    log_msg(2, &format!("hipGetDevice() -> device={}, ret={}", val, err));
    err
}

extern "C" fn wrap_hipGetDeviceCount(count: *mut c_int) -> hipError_t {
    let err = unsafe { (real().hipGetDeviceCount.unwrap())(count) };
    let val = if count.is_null() { -1 } else { unsafe { *count } };
    log_msg(2, &format!("hipGetDeviceCount() -> count={}, ret={}", val, err));
    err
}

extern "C" fn wrap_hipSetDevice(id: c_int) -> hipError_t {
    let err = unsafe { (real().hipSetDevice.unwrap())(id) };
    log_msg(2, &format!("hipSetDevice({}) -> {}", id, err));
    err
}

extern "C" fn wrap_hipDeviceSynchronize() -> hipError_t {
    let err = unsafe { (real().hipDeviceSynchronize.unwrap())() };
    log_msg(2, &format!("hipDeviceSynchronize() -> {}", err));
    err
}

extern "C" fn wrap_hipGetDeviceProperties(prop: *mut hipDeviceProp_t, id: c_int) -> hipError_t {
    let err = unsafe { (real().hipGetDeviceProperties.unwrap())(prop, id) };
    if err == 0 && !prop.is_null() {
        log_msg(2, &format!("hipGetDeviceProperties(device={}) -> ret={}", id, err));
        let name = unsafe { cstr(prop as *const c_char) };
        log_msg(2, &format!("  DeviceProps: name={}", name));
        log_msg(
            2,
            "  DeviceProps: (use hipDeviceGetAttribute logs for individual fields)",
        );
    } else {
        log_msg(2, &format!("hipGetDeviceProperties(device={}) -> ret={}", id, err));
    }
    err
}

extern "C" fn wrap_hipDeviceGetAttribute(
    value: *mut c_int,
    attr: hipDeviceAttribute_t,
    id: c_int,
) -> hipError_t {
    let err = unsafe { (real().hipDeviceGetAttribute.unwrap())(value, attr, id) };
    let val = if value.is_null() { -1 } else { unsafe { *value } };
    log_msg(
        2,
        &format!(
            "hipDeviceGetAttribute(attr={}({}), device={}) -> value={}, ret={}",
            device_attribute_name(attr),
            attr,
            id,
            val,
            err
        ),
    );
    err
}

extern "C" fn wrap_hipDeviceGetName(name: *mut c_char, len: c_int, id: c_int) -> hipError_t {
    let err = unsafe { (real().hipDeviceGetName.unwrap())(name, len, id) };
    log_msg(
        2,
        &format!("hipDeviceGetName(device={}) -> name={}, ret={}", id, unsafe { cstr(name) }, err),
    );
    err
}

extern "C" fn wrap_hipMemGetInfo(free_mem: *mut usize, total: *mut usize) -> hipError_t {
    let err = unsafe { (real().hipMemGetInfo.unwrap())(free_mem, total) };
    let f = if free_mem.is_null() { 0 } else { unsafe { *free_mem } };
    let t = if total.is_null() { 0 } else { unsafe { *total } };
    log_msg(2, &format!("hipMemGetInfo() -> free={}, total={}, ret={}", f, t, err));
    err
}

extern "C" fn wrap_hipMalloc(ptr: *mut *mut c_void, size: usize) -> hipError_t {
    let err = unsafe { (real().hipMalloc.unwrap())(ptr, size) };
    let pv = if ptr.is_null() { core::ptr::null() } else { unsafe { *ptr } };
    log_msg(2, &format!("hipMalloc(size={}) -> ptr={}, ret={}", size, p(pv), err));
    err
}

extern "C" fn wrap_hipFree(ptr: *mut c_void) -> hipError_t {
    let err = unsafe { (real().hipFree.unwrap())(ptr) };
    log_msg(2, &format!("hipFree({}) -> {}", p(ptr), err));
    err
}

extern "C" fn wrap_hipHostMalloc(ptr: *mut *mut c_void, size: usize, flags: c_uint) -> hipError_t {
    let err = unsafe { (real().hipHostMalloc.unwrap())(ptr, size, flags) };
    let pv = if ptr.is_null() { core::ptr::null() } else { unsafe { *ptr } };
    log_msg(
        2,
        &format!("hipHostMalloc(size={}, flags=0x{:x}) -> ptr={}, ret={}", size, flags, p(pv), err),
    );
    err
}

extern "C" fn wrap_hipHostFree(ptr: *mut c_void) -> hipError_t {
    let err = unsafe { (real().hipHostFree.unwrap())(ptr) };
    log_msg(2, &format!("hipHostFree({}) -> {}", p(ptr), err));
    err
}

extern "C" fn wrap_hipMemcpy(
    dst: *mut c_void,
    src: *const c_void,
    n: usize,
    kind: hipMemcpyKind,
) -> hipError_t {
    let err = unsafe { (real().hipMemcpy.unwrap())(dst, src, n, kind) };
    log_msg(
        2,
        &format!(
            "hipMemcpy(dst={}, src={}, size={}, kind={}) -> {}",
            p(dst),
            p(src),
            n,
            memcpy_kind_name(kind),
            err
        ),
    );
    err
}

extern "C" fn wrap_hipMemcpyAsync(
    dst: *mut c_void,
    src: *const c_void,
    n: usize,
    kind: hipMemcpyKind,
    stream: hipStream_t,
) -> hipError_t {
    let err = unsafe { (real().hipMemcpyAsync.unwrap())(dst, src, n, kind, stream) };
    log_msg(
        2,
        &format!(
            "hipMemcpyAsync(dst={}, src={}, size={}, kind={}, stream={}) -> {}",
            p(dst),
            p(src),
            n,
            memcpy_kind_name(kind),
            p(stream),
            err
        ),
    );
    err
}

extern "C" fn wrap_hipMemset(dst: *mut c_void, value: c_int, n: usize) -> hipError_t {
    let err = unsafe { (real().hipMemset.unwrap())(dst, value, n) };
    log_msg(2, &format!("hipMemset(dst={}, value=0x{:02x}, size={}) -> {}", p(dst), value, n, err));
    err
}

extern "C" fn wrap_hipMemsetAsync(
    dst: *mut c_void,
    value: c_int,
    n: usize,
    stream: hipStream_t,
) -> hipError_t {
    let err = unsafe { (real().hipMemsetAsync.unwrap())(dst, value, n, stream) };
    log_msg(
        2,
        &format!(
            "hipMemsetAsync(dst={}, value=0x{:02x}, size={}, stream={}) -> {}",
            p(dst),
            value,
            n,
            p(stream),
            err
        ),
    );
    err
}

extern "C" fn wrap_hipStreamCreate(stream: *mut hipStream_t) -> hipError_t {
    let err = unsafe { (real().hipStreamCreate.unwrap())(stream) };
    let sv = if stream.is_null() { core::ptr::null() } else { unsafe { *stream } };
    log_msg(2, &format!("hipStreamCreate() -> stream={}, ret={}", p(sv), err));
    err
}

extern "C" fn wrap_hipStreamCreateWithFlags(stream: *mut hipStream_t, flags: c_uint) -> hipError_t {
    let err = unsafe { (real().hipStreamCreateWithFlags.unwrap())(stream, flags) };
    let sv = if stream.is_null() { core::ptr::null() } else { unsafe { *stream } };
    log_msg(
        2,
        &format!("hipStreamCreateWithFlags(flags=0x{:x}) -> stream={}, ret={}", flags, p(sv), err),
    );
    err
}

extern "C" fn wrap_hipStreamDestroy(stream: hipStream_t) -> hipError_t {
    let err = unsafe { (real().hipStreamDestroy.unwrap())(stream) };
    log_msg(2, &format!("hipStreamDestroy(stream={}) -> {}", p(stream), err));
    err
}

extern "C" fn wrap_hipStreamSynchronize(stream: hipStream_t) -> hipError_t {
    let err = unsafe { (real().hipStreamSynchronize.unwrap())(stream) };
    log_msg(2, &format!("hipStreamSynchronize(stream={}) -> {}", p(stream), err));
    err
}

extern "C" fn wrap_hipStreamQuery(stream: hipStream_t) -> hipError_t {
    let err = unsafe { (real().hipStreamQuery.unwrap())(stream) };
    log_msg(2, &format!("hipStreamQuery(stream={}) -> {}", p(stream), err));
    err
}

extern "C" fn wrap_hipStreamWaitEvent(
    stream: hipStream_t,
    event: hipEvent_t,
    flags: c_uint,
) -> hipError_t {
    let err = unsafe { (real().hipStreamWaitEvent.unwrap())(stream, event, flags) };
    log_msg(
        2,
        &format!(
            "hipStreamWaitEvent(stream={}, event={}, flags=0x{:x}) -> {}",
            p(stream),
            p(event),
            flags,
            err
        ),
    );
    err
}

extern "C" fn wrap_hipEventCreate(event: *mut hipEvent_t) -> hipError_t {
    let err = unsafe { (real().hipEventCreate.unwrap())(event) };
    let ev = if event.is_null() { core::ptr::null() } else { unsafe { *event } };
    log_msg(2, &format!("hipEventCreate() -> event={}, ret={}", p(ev), err));
    err
}

extern "C" fn wrap_hipEventCreateWithFlags(event: *mut hipEvent_t, flags: c_uint) -> hipError_t {
    let err = unsafe { (real().hipEventCreateWithFlags.unwrap())(event, flags) };
    let ev = if event.is_null() { core::ptr::null() } else { unsafe { *event } };
    log_msg(
        2,
        &format!("hipEventCreateWithFlags(flags=0x{:x}) -> event={}, ret={}", flags, p(ev), err),
    );
    err
}

extern "C" fn wrap_hipEventDestroy(event: hipEvent_t) -> hipError_t {
    let err = unsafe { (real().hipEventDestroy.unwrap())(event) };
    log_msg(2, &format!("hipEventDestroy(event={}) -> {}", p(event), err));
    err
}

extern "C" fn wrap_hipEventRecord(event: hipEvent_t, stream: hipStream_t) -> hipError_t {
    let err = unsafe { (real().hipEventRecord.unwrap())(event, stream) };
    log_msg(2, &format!("hipEventRecord(event={}, stream={}) -> {}", p(event), p(stream), err));
    err
}

extern "C" fn wrap_hipEventSynchronize(event: hipEvent_t) -> hipError_t {
    let err = unsafe { (real().hipEventSynchronize.unwrap())(event) };
    log_msg(2, &format!("hipEventSynchronize(event={}) -> {}", p(event), err));
    err
}

extern "C" fn wrap_hipEventQuery(event: hipEvent_t) -> hipError_t {
    let err = unsafe { (real().hipEventQuery.unwrap())(event) };
    log_msg(2, &format!("hipEventQuery(event={}) -> {}", p(event), err));
    err
}

extern "C" fn wrap_hipEventElapsedTime(
    ms: *mut c_float,
    start: hipEvent_t,
    stop: hipEvent_t,
) -> hipError_t {
    let err = unsafe { (real().hipEventElapsedTime.unwrap())(ms, start, stop) };
    let v = if ms.is_null() { 0.0f32 } else { unsafe { *ms } };
    log_msg(
        2,
        &format!("hipEventElapsedTime(start={}, stop={}) -> ms={:.6}, ret={}", p(start), p(stop), v, err),
    );
    err
}

extern "C" fn wrap_hipModuleLaunchKernel(
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
    let err = unsafe {
        (real().hipModuleLaunchKernel.unwrap())(f, gx, gy, gz, bx, by, bz, shared, stream, kernel_params, extra)
    };
    log_msg(
        2,
        &format!(
            "hipModuleLaunchKernel(func={}, grid=({},{},{}), block=({},{},{}), shared={}, stream={}, extra={}) -> {}",
            p(f), gx, gy, gz, bx, by, bz, shared, p(stream), p(extra), err
        ),
    );
    err
}

extern "C" fn wrap_hipExtModuleLaunchKernel(
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
    let err = unsafe {
        (real().hipExtModuleLaunchKernel.unwrap())(
            f, gx, gy, gz, lx, ly, lz, shared, stream, kernel_params, extra, start_event, stop_event, flags,
        )
    };
    log_msg(
        2,
        &format!(
            "hipExtModuleLaunchKernel(func={}, grid=({},{},{}), block=({},{},{}), shared={}, stream={}, flags=0x{:x}) -> {}",
            p(f), gx, gy, gz, lx, ly, lz, shared, p(stream), flags, err
        ),
    );
    err
}

extern "C" fn wrap_hipLaunchKernel(
    func: *const c_void,
    num_blocks: dim3,
    dim_blocks: dim3,
    args: *mut *mut c_void,
    shared: usize,
    stream: hipStream_t,
) -> hipError_t {
    let err =
        unsafe { (real().hipLaunchKernel.unwrap())(func, num_blocks, dim_blocks, args, shared, stream) };
    log_msg(
        2,
        &format!(
            "hipLaunchKernel(func={}, grid=({},{},{}), block=({},{},{}), shared={}, stream={}) -> {}",
            p(func), num_blocks.x, num_blocks.y, num_blocks.z, dim_blocks.x, dim_blocks.y, dim_blocks.z, shared, p(stream), err
        ),
    );
    err
}

extern "C" fn wrap_hipGetLastError() -> hipError_t {
    let err = unsafe { (real().hipGetLastError.unwrap())() };
    if err != 0 {
        log_msg(1, &format!("hipGetLastError() -> {} (ERROR)", err));
    } else {
        log_msg(3, "hipGetLastError() -> 0");
    }
    err
}

extern "C" fn wrap_hipPeekAtLastError() -> hipError_t {
    let err = unsafe { (real().hipPeekAtLastError.unwrap())() };
    if err != 0 {
        log_msg(1, &format!("hipPeekAtLastError() -> {} (ERROR)", err));
    } else {
        log_msg(3, "hipPeekAtLastError() -> 0");
    }
    err
}

extern "C" fn wrap___hipRegisterFatBinary(data: *const c_void) -> *mut *mut c_void {
    let result = unsafe { (real().__hipRegisterFatBinary.unwrap())(data) };
    log_msg(2, &format!("__hipRegisterFatBinary(data={}) -> handle={}", p(data), p(result)));
    result
}

extern "C" fn wrap___hipUnregisterFatBinary(handle: *mut *mut c_void) {
    log_msg(2, &format!("__hipUnregisterFatBinary(handle={})", p(handle)));
    unsafe { (real().__hipUnregisterFatBinary.unwrap())(handle) };
}

extern "C" fn wrap___hipRegisterFunction(
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
    log_msg(
        2,
        &format!(
            "__hipRegisterFunction(handle={}, host={}, device={})",
            p(handle),
            p(host_fun),
            unsafe { cstr(device_name) }
        ),
    );
    unsafe {
        (real().__hipRegisterFunction.unwrap())(
            handle, host_fun, device_fun, device_name, thread_limit, tid, bid, block_dim, grid_dim, w_size,
        )
    };
}

extern "C" fn wrap___hipRegisterVar(
    handle: *mut *mut c_void,
    host_var: *mut c_char,
    device_address: *mut c_char,
    device_name: *const c_char,
    ext: c_int,
    size: usize,
    constant: c_int,
    global: c_int,
) {
    log_msg(
        2,
        &format!("__hipRegisterVar(handle={}, name={}, size={})", p(handle), unsafe { cstr(device_name) }, size),
    );
    unsafe {
        (real().__hipRegisterVar.unwrap())(handle, host_var, device_address, device_name, ext, size, constant, global)
    };
}

// ---------------------------------------------------------------------------
// Interceptor entry points.
// ---------------------------------------------------------------------------

/// `int atoi`-like: leading optional sign + digits, 0 otherwise (matches C).
fn atoi(s: &str) -> c_int {
    let s = s.trim_start();
    let (neg, rest) = match s.strip_prefix('-') {
        Some(r) => (true, r),
        None => (false, s.strip_prefix('+').unwrap_or(s)),
    };
    let digits: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
    let v: c_int = digits.parse().unwrap_or(0);
    if neg { -v } else { v }
}

#[no_mangle]
pub unsafe extern "C" fn hip_interceptor_init(
    real_functions: *mut HipFunctionTable,
) -> *mut HipFunctionTable {
    G_REAL.store(real_functions, Relaxed);

    // Open the log destination (HIP_LOG_FILE or stderr).
    let (file, owns_file) = match std::env::var("HIP_LOG_FILE") {
        Ok(path) if !path.is_empty() => {
            let cpath = std::ffi::CString::new(path.clone()).unwrap();
            let f = libc::fopen(cpath.as_ptr(), c"w".as_ptr());
            if f.is_null() {
                eprintln!("hip_logging: failed to open log file: {path}");
                (libc::fdopen(2, c"w".as_ptr()), false)
            } else {
                (f, true)
            }
        }
        _ => (libc::fdopen(2, c"w".as_ptr()), false),
    };
    let level = match std::env::var("HIP_LOG_LEVEL") {
        Ok(s) => atoi(&s),
        Err(_) => 2,
    };
    {
        let mut g = LOG.lock().unwrap();
        g.file = file;
        g.level = level;
        g.owns_file = owns_file;
    }
    log_msg(1, &format!("HIP Logging initialized (level={})", level));

    // Copy the real table, then override the entries we wrap.
    let mut w: HipFunctionTable = *real_functions;
    w.hipInit = Some(wrap_hipInit);
    w.hipDriverGetVersion = Some(wrap_hipDriverGetVersion);
    w.hipRuntimeGetVersion = Some(wrap_hipRuntimeGetVersion);
    w.hipGetDevice = Some(wrap_hipGetDevice);
    w.hipGetDeviceCount = Some(wrap_hipGetDeviceCount);
    w.hipSetDevice = Some(wrap_hipSetDevice);
    w.hipDeviceSynchronize = Some(wrap_hipDeviceSynchronize);
    w.hipGetDeviceProperties = Some(wrap_hipGetDeviceProperties);
    w.hipDeviceGetAttribute = Some(wrap_hipDeviceGetAttribute);
    w.hipDeviceGetName = Some(wrap_hipDeviceGetName);
    w.hipMalloc = Some(wrap_hipMalloc);
    w.hipFree = Some(wrap_hipFree);
    w.hipHostMalloc = Some(wrap_hipHostMalloc);
    w.hipHostFree = Some(wrap_hipHostFree);
    w.hipMemGetInfo = Some(wrap_hipMemGetInfo);
    w.hipMemcpy = Some(wrap_hipMemcpy);
    w.hipMemcpyAsync = Some(wrap_hipMemcpyAsync);
    w.hipMemset = Some(wrap_hipMemset);
    w.hipMemsetAsync = Some(wrap_hipMemsetAsync);
    w.hipStreamCreate = Some(wrap_hipStreamCreate);
    w.hipStreamCreateWithFlags = Some(wrap_hipStreamCreateWithFlags);
    w.hipStreamDestroy = Some(wrap_hipStreamDestroy);
    w.hipStreamSynchronize = Some(wrap_hipStreamSynchronize);
    w.hipStreamQuery = Some(wrap_hipStreamQuery);
    w.hipStreamWaitEvent = Some(wrap_hipStreamWaitEvent);
    w.hipEventCreate = Some(wrap_hipEventCreate);
    w.hipEventCreateWithFlags = Some(wrap_hipEventCreateWithFlags);
    w.hipEventDestroy = Some(wrap_hipEventDestroy);
    w.hipEventRecord = Some(wrap_hipEventRecord);
    w.hipEventSynchronize = Some(wrap_hipEventSynchronize);
    w.hipEventQuery = Some(wrap_hipEventQuery);
    w.hipEventElapsedTime = Some(wrap_hipEventElapsedTime);
    w.hipModuleLaunchKernel = Some(wrap_hipModuleLaunchKernel);
    w.hipExtModuleLaunchKernel = Some(wrap_hipExtModuleLaunchKernel);
    w.hipLaunchKernel = Some(wrap_hipLaunchKernel);
    w.__hipRegisterFatBinary = Some(wrap___hipRegisterFatBinary);
    w.__hipUnregisterFatBinary = Some(wrap___hipUnregisterFatBinary);
    w.__hipRegisterFunction = Some(wrap___hipRegisterFunction);
    w.__hipRegisterVar = Some(wrap___hipRegisterVar);
    w.hipGetLastError = Some(wrap_hipGetLastError);
    w.hipPeekAtLastError = Some(wrap_hipPeekAtLastError);

    Box::into_raw(Box::new(w))
}

#[no_mangle]
pub unsafe extern "C" fn hip_interceptor_shutdown() {
    log_msg(1, "HIP Logging shutting down");
    let mut g = LOG.lock().unwrap();
    if g.owns_file && !g.file.is_null() {
        libc::fclose(g.file);
        g.file = core::ptr::null_mut();
    }
}
