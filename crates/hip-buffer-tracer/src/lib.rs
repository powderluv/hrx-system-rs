//! Rust port of `libhrx/src/passthrough/hip_buffer_tracer.c`.
//!
//! A stateful interceptor loaded by `libhip_intercept.so`. It tracks HIP memory
//! allocations and kernel registrations, and (optionally) dumps buffer contents
//! or hashes around kernel launches. Wrappers forward to the real table and
//! emit trace lines in the exact textual format of the C tracer, so the Rust
//! and C tracers are byte-for-byte comparable.
#![allow(non_snake_case)]

use core::ffi::{c_char, c_int, c_uint, c_void};
use std::ffi::CStr;
use std::sync::Mutex;

use hip_function_table::*;
use libc::{c_void as lc_void, size_t, FILE};

const MAX_TRACKED_BUFFERS: usize = 65536;

// ---------------------------------------------------------------------------
// Buffer / kernel tracking state.
// ---------------------------------------------------------------------------
#[derive(Clone, Copy, PartialEq)]
enum BufferType {
    Device = 0,
    Host = 1,
    #[allow(dead_code)]
    Managed = 2,
}

fn buffer_type_name(t: BufferType) -> &'static str {
    match t {
        BufferType::Device => "device",
        BufferType::Host => "host",
        BufferType::Managed => "managed",
    }
}

#[derive(Clone, Copy)]
struct TrackedBuffer {
    ptr: *mut c_void,
    size: usize,
    btype: BufferType,
    in_use: bool,
    alloc_id: u64,
}

struct BufferTable {
    buffers: Vec<TrackedBuffer>,
    count: usize,
    next_alloc_id: u64,
}

struct KernelInfo {
    host_func: *mut c_void,
    name: String,
}

/// All mutable interceptor state, behind one mutex (the C uses several; a single
/// lock is sufficient here and preserves observable ordering of trace lines).
struct State {
    real: *mut HipFunctionTable,
    wrapper: *mut HipFunctionTable,
    trace_file: *mut FILE,
    trace_level: c_int,
    trace_sync: bool,
    trace_dump: c_int, // 0=none, 1=full, 2=hash
    trace_dump_max: usize,
    kernel_filter: Option<String>,
    kernel_count_limit: c_int,
    kernel_count: c_int,
    kernel_full_dump_list: Option<String>,
    owns_file: bool,
    buffers: BufferTable,
    kernels: Vec<KernelInfo>,
}
unsafe impl Send for State {}

static STATE: Mutex<Option<State>> = Mutex::new(None);

// Helpers ------------------------------------------------------------------
#[inline]
fn real_table(st: &State) -> &HipFunctionTable {
    unsafe { &*st.real }
}

fn now_ts() -> (i64, i64) {
    let mut ts = libc::timespec { tv_sec: 0, tv_nsec: 0 };
    unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut ts) };
    (ts.tv_sec as i64, ts.tv_nsec as i64)
}

/// `trace_msg`: timestamped, newline-terminated, level-gated, flushed.
fn trace_msg(st: &State, level: c_int, msg: &str) {
    if level > st.trace_level || st.trace_file.is_null() {
        return;
    }
    let (s, ns) = now_ts();
    let line = format!("[{}.{:06}] {}\n", s, ns / 1000, msg);
    unsafe {
        libc::fwrite(line.as_ptr() as *const lc_void, 1, line.len() as size_t, st.trace_file);
        libc::fflush(st.trace_file);
    }
}

/// Raw write to the trace file with no timestamp (matches the C `fprintf`s used
/// directly in the dump path).
fn raw_write(st: &State, s: &str) {
    if st.trace_file.is_null() {
        return;
    }
    unsafe {
        libc::fwrite(s.as_ptr() as *const lc_void, 1, s.len() as size_t, st.trace_file);
    }
}

fn p<T>(ptr: *const T) -> String {
    if ptr.is_null() {
        "(nil)".to_string()
    } else {
        format!("0x{:x}", ptr as usize)
    }
}

unsafe fn cstr_or(ptr: *const c_char, dflt: &str) -> String {
    if ptr.is_null() {
        dflt.to_string()
    } else {
        CStr::from_ptr(ptr).to_string_lossy().into_owned()
    }
}

fn memcpy_kind_name(kind: hipMemcpyKind) -> &'static str {
    hip_function_table::memcpy_kind_name(kind)
}

/// FNV-1a 64-bit, identical to the C `compute_hash`.
fn compute_hash(data: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for &b in data {
        hash ^= b as u64;
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

// Buffer table ops ---------------------------------------------------------
fn buffer_table_add(st: &mut State, ptr: *mut c_void, size: usize, btype: BufferType) {
    if ptr.is_null() || size == 0 {
        return;
    }
    // Find a free slot (linear, matching the C semantics / slot reuse).
    let slot = st.buffers.buffers.iter().position(|b| !b.in_use);
    let slot = match slot {
        Some(s) => s,
        None => {
            let msg = format!("WARNING: buffer table full, cannot track {}", p(ptr));
            trace_msg(st, 1, &msg);
            return;
        }
    };
    let id = st.buffers.next_alloc_id;
    st.buffers.next_alloc_id += 1;
    st.buffers.buffers[slot] = TrackedBuffer {
        ptr,
        size,
        btype,
        in_use: true,
        alloc_id: id,
    };
    st.buffers.count += 1;
}

fn buffer_table_remove(st: &mut State, ptr: *mut c_void) {
    if ptr.is_null() {
        return;
    }
    if let Some(b) = st.buffers.buffers.iter_mut().find(|b| b.in_use && b.ptr == ptr) {
        b.in_use = false;
        st.buffers.count -= 1;
    }
}

// Kernel table ops ---------------------------------------------------------
fn kernel_table_add(st: &mut State, host_func: *mut c_void, name: Option<String>) {
    if host_func.is_null() {
        return;
    }
    if st.kernels.iter().any(|k| k.host_func == host_func) {
        return;
    }
    let name = name.unwrap_or_else(|| format!("kernel_{}", p(host_func)));
    st.kernels.push(KernelInfo { host_func, name });
}

fn kernel_table_get_name(st: &State, host_func: *mut c_void) -> Option<String> {
    st.kernels
        .iter()
        .find(|k| k.host_func == host_func)
        .map(|k| k.name.clone())
}

// Buffer dumping -----------------------------------------------------------
fn dump_buffer_hex(st: &State, data: &[u8], size: usize, max_bytes: usize) {
    let dump_size = size.min(max_bytes);
    let mut out = String::from("    ");
    for i in 0..dump_size {
        out.push_str(&format!("{:02x}", data[i]));
        if (i + 1) % 32 == 0 {
            out.push_str("\n    ");
        } else if (i + 1) % 4 == 0 {
            out.push(' ');
        }
    }
    if size > max_bytes {
        out.push_str(&format!("... ({} more bytes)", size - max_bytes));
    }
    out.push('\n');
    raw_write(st, &out);
}

fn dump_all_buffers_ex(st: &State, label: &str, force_full_dump: bool) {
    if st.trace_dump == 0 {
        return;
    }
    let suffix = if force_full_dump { " [FULL DUMP]" } else { "" };
    trace_msg(
        st,
        3,
        &format!("=== Buffer Dump: {} ({} buffers){} ===", label, st.buffers.count, suffix),
    );

    let mut staging: Vec<u8> = Vec::new();
    for buf in st.buffers.buffers.iter() {
        if !buf.in_use || buf.size == 0 {
            continue;
        }
        let mut read_size = buf.size;
        let max_dump = if force_full_dump { buf.size } else { st.trace_dump_max };
        if !force_full_dump && st.trace_dump_max > 0 && read_size > st.trace_dump_max {
            read_size = st.trace_dump_max;
        }

        // Read the bytes: device buffers staged through host via hipMemcpy.
        let data: &[u8] = if buf.btype == BufferType::Device {
            staging.resize(read_size, 0);
            let err = unsafe {
                (real_table(st).hipMemcpy.unwrap())(
                    staging.as_mut_ptr() as *mut c_void,
                    buf.ptr as *const c_void,
                    read_size,
                    2, // hipMemcpyDeviceToHost
                )
            };
            if err != 0 {
                trace_msg(
                    st,
                    1,
                    &format!(
                        "  [alloc {}] {} {} size={} - MEMCPY FAILED: {}",
                        buf.alloc_id, buffer_type_name(buf.btype), p(buf.ptr), buf.size, err
                    ),
                );
                continue;
            }
            &staging[..read_size]
        } else {
            unsafe { core::slice::from_raw_parts(buf.ptr as *const u8, read_size) }
        };

        if st.trace_dump == 2 && !force_full_dump {
            let hash = compute_hash(data);
            raw_write(
                st,
                &format!(
                    "  [alloc {}] {} {} size={} hash=0x{:016x}\n",
                    buf.alloc_id, buffer_type_name(buf.btype), p(buf.ptr), buf.size, hash
                ),
            );
        } else {
            raw_write(
                st,
                &format!(
                    "  [alloc {}] {} {} size={}:\n",
                    buf.alloc_id, buffer_type_name(buf.btype), p(buf.ptr), buf.size
                ),
            );
            dump_buffer_hex(st, data, buf.size, max_dump);
        }
    }
    if !st.trace_file.is_null() {
        unsafe { libc::fflush(st.trace_file) };
    }
}

// Kernel trace decisions ---------------------------------------------------
fn should_trace_kernel(st: &State, kernel_name: Option<&str>) -> bool {
    if st.kernel_count_limit > 0 && st.kernel_count >= st.kernel_count_limit {
        return false;
    }
    if let Some(filter) = &st.kernel_filter {
        if !filter.is_empty() {
            match kernel_name {
                Some(n) if n.contains(filter.as_str()) => {}
                _ => return false,
            }
        }
    }
    true
}

fn should_full_dump_kernel(st: &State, kernel_name: Option<&str>) -> bool {
    let (list, name) = match (&st.kernel_full_dump_list, kernel_name) {
        (Some(l), Some(n)) if !l.is_empty() => (l, n),
        _ => return false,
    };
    for token in list.split(':') {
        if !token.is_empty() && name.contains(token) {
            trace_msg(
                st,
                3,
                &format!("Full dump match: kernel='{}' matches token='{}'", name, token),
            );
            return true;
        }
    }
    false
}

// ---------------------------------------------------------------------------
// Wrapper helpers. Each wrapper locks STATE, calls the real fn (released lock
// during the call is NOT needed — the C holds no lock across the real call
// either, but its globals are unsynchronized; we keep the lock for the table
// mutations and drop it before the real call where the C would deadlock — here
// the real calls don't re-enter us, so holding is safe and simpler).
// ---------------------------------------------------------------------------
macro_rules! with_state {
    (|$st:ident| $body:block) => {{
        let mut guard = STATE.lock().unwrap();
        let $st = guard.as_mut().unwrap();
        $body
    }};
}

// Memory allocation
extern "C" fn wrap_hipMalloc(ptr: *mut *mut c_void, size: usize) -> hipError_t {
    with_state!(|st| {
        let err = unsafe { (real_table(st).hipMalloc.unwrap())(ptr, size) };
        if err == 0 && !ptr.is_null() {
            let pv = unsafe { *ptr };
            if !pv.is_null() {
                buffer_table_add(st, pv, size, BufferType::Device);
            }
        }
        let pv = if ptr.is_null() { core::ptr::null_mut() } else { unsafe { *ptr } };
        trace_msg(st, 2, &format!("hipMalloc(size={}) -> ptr={}, ret={}", size, p(pv), err));
        err
    })
}

extern "C" fn wrap_hipFree(ptr: *mut c_void) -> hipError_t {
    with_state!(|st| {
        buffer_table_remove(st, ptr);
        let err = unsafe { (real_table(st).hipFree.unwrap())(ptr) };
        trace_msg(st, 2, &format!("hipFree({}) -> {}", p(ptr), err));
        err
    })
}

extern "C" fn wrap_hipHostMalloc(ptr: *mut *mut c_void, size: usize, flags: c_uint) -> hipError_t {
    with_state!(|st| {
        let err = unsafe { (real_table(st).hipHostMalloc.unwrap())(ptr, size, flags) };
        if err == 0 && !ptr.is_null() {
            let pv = unsafe { *ptr };
            if !pv.is_null() {
                buffer_table_add(st, pv, size, BufferType::Host);
            }
        }
        let pv = if ptr.is_null() { core::ptr::null_mut() } else { unsafe { *ptr } };
        trace_msg(st, 2, &format!("hipHostMalloc(size={}, flags=0x{:x}) -> ptr={}, ret={}", size, flags, p(pv), err));
        err
    })
}

extern "C" fn wrap_hipHostFree(ptr: *mut c_void) -> hipError_t {
    with_state!(|st| {
        buffer_table_remove(st, ptr);
        let err = unsafe { (real_table(st).hipHostFree.unwrap())(ptr) };
        trace_msg(st, 2, &format!("hipHostFree({}) -> {}", p(ptr), err));
        err
    })
}

// Memory ops
extern "C" fn wrap_hipMemcpy(dst: *mut c_void, src: *const c_void, n: usize, kind: hipMemcpyKind) -> hipError_t {
    with_state!(|st| {
        let err = unsafe { (real_table(st).hipMemcpy.unwrap())(dst, src, n, kind) };
        trace_msg(st, 2, &format!("hipMemcpy(dst={}, src={}, size={}, kind={}) -> {}", p(dst), p(src), n, memcpy_kind_name(kind), err));
        err
    })
}

extern "C" fn wrap_hipMemcpyAsync(dst: *mut c_void, src: *const c_void, n: usize, kind: hipMemcpyKind, stream: hipStream_t) -> hipError_t {
    with_state!(|st| {
        let err = unsafe { (real_table(st).hipMemcpyAsync.unwrap())(dst, src, n, kind, stream) };
        trace_msg(st, 2, &format!("hipMemcpyAsync(dst={}, src={}, size={}, kind={}, stream={}) -> {}", p(dst), p(src), n, memcpy_kind_name(kind), p(stream), err));
        err
    })
}

extern "C" fn wrap_hipMemset(dst: *mut c_void, value: c_int, n: usize) -> hipError_t {
    with_state!(|st| {
        let err = unsafe { (real_table(st).hipMemset.unwrap())(dst, value, n) };
        trace_msg(st, 2, &format!("hipMemset(dst={}, value=0x{:02x}, size={}) -> {}", p(dst), value, n, err));
        err
    })
}

extern "C" fn wrap_hipMemsetAsync(dst: *mut c_void, value: c_int, n: usize, stream: hipStream_t) -> hipError_t {
    with_state!(|st| {
        let err = unsafe { (real_table(st).hipMemsetAsync.unwrap())(dst, value, n, stream) };
        trace_msg(st, 2, &format!("hipMemsetAsync(dst={}, value=0x{:02x}, size={}, stream={}) -> {}", p(dst), value, n, p(stream), err));
        err
    })
}

// Device management
extern "C" fn wrap_hipInit(flags: c_uint) -> hipError_t {
    with_state!(|st| {
        let err = unsafe { (real_table(st).hipInit.unwrap())(flags) };
        trace_msg(st, 2, &format!("hipInit(flags=0x{:x}) -> {}", flags, err));
        err
    })
}

extern "C" fn wrap_hipGetDevice(device_id: *mut c_int) -> hipError_t {
    with_state!(|st| {
        let err = unsafe { (real_table(st).hipGetDevice.unwrap())(device_id) };
        let v = if device_id.is_null() { -1 } else { unsafe { *device_id } };
        trace_msg(st, 2, &format!("hipGetDevice() -> device={}, ret={}", v, err));
        err
    })
}

extern "C" fn wrap_hipGetDeviceCount(count: *mut c_int) -> hipError_t {
    with_state!(|st| {
        let err = unsafe { (real_table(st).hipGetDeviceCount.unwrap())(count) };
        let v = if count.is_null() { -1 } else { unsafe { *count } };
        trace_msg(st, 2, &format!("hipGetDeviceCount() -> count={}, ret={}", v, err));
        err
    })
}

extern "C" fn wrap_hipSetDevice(device_id: c_int) -> hipError_t {
    with_state!(|st| {
        let err = unsafe { (real_table(st).hipSetDevice.unwrap())(device_id) };
        trace_msg(st, 2, &format!("hipSetDevice({}) -> {}", device_id, err));
        err
    })
}

extern "C" fn wrap_hipDeviceSynchronize() -> hipError_t {
    with_state!(|st| {
        let err = unsafe { (real_table(st).hipDeviceSynchronize.unwrap())() };
        trace_msg(st, 2, &format!("hipDeviceSynchronize() -> {}", err));
        err
    })
}

// Stream management
extern "C" fn wrap_hipStreamCreate(stream: *mut hipStream_t) -> hipError_t {
    with_state!(|st| {
        let err = unsafe { (real_table(st).hipStreamCreate.unwrap())(stream) };
        let sv = if stream.is_null() { core::ptr::null_mut() } else { unsafe { *stream } };
        trace_msg(st, 2, &format!("hipStreamCreate() -> stream={}, ret={}", p(sv), err));
        err
    })
}

extern "C" fn wrap_hipStreamCreateWithFlags(stream: *mut hipStream_t, flags: c_uint) -> hipError_t {
    with_state!(|st| {
        let err = unsafe { (real_table(st).hipStreamCreateWithFlags.unwrap())(stream, flags) };
        let sv = if stream.is_null() { core::ptr::null_mut() } else { unsafe { *stream } };
        trace_msg(st, 2, &format!("hipStreamCreateWithFlags(flags=0x{:x}) -> stream={}, ret={}", flags, p(sv), err));
        err
    })
}

extern "C" fn wrap_hipStreamDestroy(stream: hipStream_t) -> hipError_t {
    with_state!(|st| {
        let err = unsafe { (real_table(st).hipStreamDestroy.unwrap())(stream) };
        trace_msg(st, 2, &format!("hipStreamDestroy(stream={}) -> {}", p(stream), err));
        err
    })
}

extern "C" fn wrap_hipStreamSynchronize(stream: hipStream_t) -> hipError_t {
    with_state!(|st| {
        let err = unsafe { (real_table(st).hipStreamSynchronize.unwrap())(stream) };
        trace_msg(st, 2, &format!("hipStreamSynchronize(stream={}) -> {}", p(stream), err));
        err
    })
}

// Event management
extern "C" fn wrap_hipEventCreate(event: *mut hipEvent_t) -> hipError_t {
    with_state!(|st| {
        let err = unsafe { (real_table(st).hipEventCreate.unwrap())(event) };
        let ev = if event.is_null() { core::ptr::null_mut() } else { unsafe { *event } };
        trace_msg(st, 2, &format!("hipEventCreate() -> event={}, ret={}", p(ev), err));
        err
    })
}

extern "C" fn wrap_hipEventCreateWithFlags(event: *mut hipEvent_t, flags: c_uint) -> hipError_t {
    with_state!(|st| {
        let err = unsafe { (real_table(st).hipEventCreateWithFlags.unwrap())(event, flags) };
        let ev = if event.is_null() { core::ptr::null_mut() } else { unsafe { *event } };
        trace_msg(st, 2, &format!("hipEventCreateWithFlags(flags=0x{:x}) -> event={}, ret={}", flags, p(ev), err));
        err
    })
}

extern "C" fn wrap_hipEventDestroy(event: hipEvent_t) -> hipError_t {
    with_state!(|st| {
        let err = unsafe { (real_table(st).hipEventDestroy.unwrap())(event) };
        trace_msg(st, 2, &format!("hipEventDestroy(event={}) -> {}", p(event), err));
        err
    })
}

extern "C" fn wrap_hipEventRecord(event: hipEvent_t, stream: hipStream_t) -> hipError_t {
    with_state!(|st| {
        let err = unsafe { (real_table(st).hipEventRecord.unwrap())(event, stream) };
        trace_msg(st, 2, &format!("hipEventRecord(event={}, stream={}) -> {}", p(event), p(stream), err));
        err
    })
}

extern "C" fn wrap_hipEventSynchronize(event: hipEvent_t) -> hipError_t {
    with_state!(|st| {
        let err = unsafe { (real_table(st).hipEventSynchronize.unwrap())(event) };
        trace_msg(st, 2, &format!("hipEventSynchronize(event={}) -> {}", p(event), err));
        err
    })
}

// Kernel launch (with optional buffer sync+dump)
#[allow(clippy::too_many_arguments)]
extern "C" fn wrap_hipModuleLaunchKernel(
    f: hipFunction_t,
    gx: c_uint, gy: c_uint, gz: c_uint,
    bx: c_uint, by: c_uint, bz: c_uint,
    shared: c_uint, stream: hipStream_t,
    kernel_params: *mut *mut c_void, extra: *mut *mut c_void,
) -> hipError_t {
    with_state!(|st| {
        let name = kernel_table_get_name(st, f);
        let do_trace = should_trace_kernel(st, name.as_deref());
        let do_full = should_full_dump_kernel(st, name.as_deref());
        if do_trace && st.trace_sync {
            unsafe { (real_table(st).hipDeviceSynchronize.unwrap())() };
            let label = format!("BEFORE kernel #{}: {}", st.kernel_count, name.as_deref().unwrap_or("(unknown)"));
            dump_all_buffers_ex(st, &label, do_full);
        }
        trace_msg(st, 2, &format!(
            "hipModuleLaunchKernel(func={} [{}], grid=({},{},{}), block=({},{},{}), shared={}, stream={})",
            p(f), name.as_deref().unwrap_or("?"), gx, gy, gz, bx, by, bz, shared, p(stream)));
        let err = unsafe {
            (real_table(st).hipModuleLaunchKernel.unwrap())(f, gx, gy, gz, bx, by, bz, shared, stream, kernel_params, extra)
        };
        if do_trace && st.trace_sync {
            unsafe { (real_table(st).hipDeviceSynchronize.unwrap())() };
            let label = format!("AFTER kernel #{}: {} (ret={})", st.kernel_count, name.as_deref().unwrap_or("(unknown)"), err);
            dump_all_buffers_ex(st, &label, do_full);
        }
        if do_trace {
            st.kernel_count += 1;
        }
        trace_msg(st, 2, &format!("  -> {}", err));
        err
    })
}

extern "C" fn wrap_hipLaunchKernel(
    function_address: *const c_void,
    num_blocks: dim3, dim_blocks: dim3,
    args: *mut *mut c_void, shared: usize, stream: hipStream_t,
) -> hipError_t {
    with_state!(|st| {
        let name = kernel_table_get_name(st, function_address as *mut c_void);
        let do_trace = should_trace_kernel(st, name.as_deref());
        let do_full = should_full_dump_kernel(st, name.as_deref());
        if do_trace && st.trace_sync {
            unsafe { (real_table(st).hipDeviceSynchronize.unwrap())() };
            let label = format!("BEFORE kernel #{}: {}", st.kernel_count, name.as_deref().unwrap_or("(unknown)"));
            dump_all_buffers_ex(st, &label, do_full);
        }
        trace_msg(st, 2, &format!(
            "hipLaunchKernel(func={} [{}], grid=({},{},{}), block=({},{},{}), shared={}, stream={})",
            p(function_address), name.as_deref().unwrap_or("?"),
            num_blocks.x, num_blocks.y, num_blocks.z, dim_blocks.x, dim_blocks.y, dim_blocks.z, shared, p(stream)));
        let err = unsafe {
            (real_table(st).hipLaunchKernel.unwrap())(function_address, num_blocks, dim_blocks, args, shared, stream)
        };
        if do_trace && st.trace_sync {
            unsafe { (real_table(st).hipDeviceSynchronize.unwrap())() };
            let label = format!("AFTER kernel #{}: {} (ret={})", st.kernel_count, name.as_deref().unwrap_or("(unknown)"), err);
            dump_all_buffers_ex(st, &label, do_full);
        }
        if do_trace {
            st.kernel_count += 1;
        }
        trace_msg(st, 2, &format!("  -> {}", err));
        err
    })
}

#[allow(clippy::too_many_arguments)]
extern "C" fn wrap_hipExtModuleLaunchKernel(
    f: hipFunction_t,
    gx: c_uint, gy: c_uint, gz: c_uint,
    lx: c_uint, ly: c_uint, lz: c_uint,
    shared: usize, stream: hipStream_t,
    kernel_params: *mut *mut c_void, extra: *mut *mut c_void,
    start_event: hipEvent_t, stop_event: hipEvent_t, flags: c_uint,
) -> hipError_t {
    with_state!(|st| {
        let name = kernel_table_get_name(st, f);
        let do_trace = should_trace_kernel(st, name.as_deref());
        let do_full = should_full_dump_kernel(st, name.as_deref());
        if do_trace && st.trace_sync {
            unsafe { (real_table(st).hipDeviceSynchronize.unwrap())() };
            let label = format!("BEFORE kernel #{}: {}", st.kernel_count, name.as_deref().unwrap_or("(unknown)"));
            dump_all_buffers_ex(st, &label, do_full);
        }
        trace_msg(st, 2, &format!(
            "hipExtModuleLaunchKernel(func={} [{}], globalSize=({},{},{}), localSize=({},{},{}), shared={}, stream={}, flags=0x{:x})",
            p(f), name.as_deref().unwrap_or("?"), gx, gy, gz, lx, ly, lz, shared, p(stream), flags));
        let err = unsafe {
            (real_table(st).hipExtModuleLaunchKernel.unwrap())(f, gx, gy, gz, lx, ly, lz, shared, stream, kernel_params, extra, start_event, stop_event, flags)
        };
        if do_trace && st.trace_sync {
            unsafe { (real_table(st).hipDeviceSynchronize.unwrap())() };
            let label = format!("AFTER kernel #{}: {} (ret={})", st.kernel_count, name.as_deref().unwrap_or("(unknown)"), err);
            dump_all_buffers_ex(st, &label, do_full);
        }
        if do_trace {
            st.kernel_count += 1;
        }
        trace_msg(st, 2, &format!("  -> {}", err));
        err
    })
}

// Fat binary registration
extern "C" fn wrap___hipRegisterFatBinary(data: *const c_void) -> *mut *mut c_void {
    with_state!(|st| {
        let result = unsafe { (real_table(st).__hipRegisterFatBinary.unwrap())(data) };
        trace_msg(st, 2, &format!("__hipRegisterFatBinary(data={}) -> handle={}", p(data), p(result)));
        result
    })
}

extern "C" fn wrap___hipUnregisterFatBinary(handle: *mut *mut c_void) {
    with_state!(|st| {
        trace_msg(st, 2, &format!("__hipUnregisterFatBinary(handle={})", p(handle)));
        unsafe { (real_table(st).__hipUnregisterFatBinary.unwrap())(handle) };
    })
}

#[allow(clippy::too_many_arguments)]
extern "C" fn wrap___hipRegisterFunction(
    handle: *mut *mut c_void, host_fun: *const c_char, device_fun: *mut c_char,
    device_name: *const c_char, thread_limit: c_int, tid: *mut c_void, bid: *mut c_void,
    block_dim: *mut dim3, grid_dim: *mut dim3, w_size: *mut c_int,
) {
    with_state!(|st| {
        let name = if device_name.is_null() { None } else { Some(unsafe { cstr_or(device_name, "") }) };
        kernel_table_add(st, host_fun as *mut c_void, name);
        trace_msg(st, 2, &format!(
            "__hipRegisterFunction(handle={}, host={}, device={})",
            p(handle), p(host_fun), unsafe { cstr_or(device_name, "(null)") }));
        unsafe {
            (real_table(st).__hipRegisterFunction.unwrap())(handle, host_fun, device_fun, device_name, thread_limit, tid, bid, block_dim, grid_dim, w_size);
        }
    })
}

extern "C" fn wrap___hipRegisterVar(
    handle: *mut *mut c_void, host_var: *mut c_char, device_address: *mut c_char,
    device_name: *const c_char, ext: c_int, size: usize, constant: c_int, global: c_int,
) {
    with_state!(|st| {
        trace_msg(st, 2, &format!(
            "__hipRegisterVar(handle={}, name={}, size={})",
            p(handle), unsafe { cstr_or(device_name, "(null)") }, size));
        unsafe {
            (real_table(st).__hipRegisterVar.unwrap())(handle, host_var, device_address, device_name, ext, size, constant, global);
        }
    })
}

// ---------------------------------------------------------------------------
// Interceptor entry points.
// ---------------------------------------------------------------------------
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

fn atol(s: &str) -> i64 {
    let s = s.trim_start();
    let (neg, rest) = match s.strip_prefix('-') {
        Some(r) => (true, r),
        None => (false, s.strip_prefix('+').unwrap_or(s)),
    };
    let digits: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
    let v: i64 = digits.parse().unwrap_or(0);
    if neg { -v } else { v }
}

#[no_mangle]
pub unsafe extern "C" fn hip_interceptor_init(
    real_functions: *mut HipFunctionTable,
) -> *mut HipFunctionTable {
    // Open trace destination.
    let (file, owns_file) = match std::env::var("HIP_TRACE_FILE") {
        Ok(path) if !path.is_empty() => {
            let cp = std::ffi::CString::new(path.clone()).unwrap();
            let f = libc::fopen(cp.as_ptr(), c"w".as_ptr());
            if f.is_null() {
                eprintln!("hip_buffer_tracer: failed to open trace file: {path}");
                (libc::fdopen(2, c"w".as_ptr()), false)
            } else {
                (f, true)
            }
        }
        _ => (libc::fdopen(2, c"w".as_ptr()), false),
    };

    let getenv_int = |k: &str, dflt: c_int| std::env::var(k).map(|s| atoi(&s)).unwrap_or(dflt);

    let trace_level = getenv_int("HIP_TRACE_LEVEL", 2);
    let trace_sync = std::env::var("HIP_TRACE_SYNC").map(|s| atoi(&s) != 0).unwrap_or(false);
    let trace_dump = getenv_int("HIP_TRACE_DUMP", 0);
    let trace_dump_max = std::env::var("HIP_TRACE_DUMP_MAX").map(|s| atol(&s) as usize).unwrap_or(1024);
    let kernel_filter = std::env::var("HIP_TRACE_KERNEL_FILTER").ok();
    let kernel_count_limit = getenv_int("HIP_TRACE_KERNEL_COUNT", 0);
    let kernel_full_dump_list = std::env::var("HIP_TRACE_KERNEL_FULL_DUMP").ok().filter(|s| !s.is_empty());

    // Build the wrapper table: copy the real table, override selected slots.
    let mut wrapper: HipFunctionTable = *real_functions;
    wrapper.hipInit = Some(wrap_hipInit);
    wrapper.hipGetDevice = Some(wrap_hipGetDevice);
    wrapper.hipGetDeviceCount = Some(wrap_hipGetDeviceCount);
    wrapper.hipSetDevice = Some(wrap_hipSetDevice);
    wrapper.hipDeviceSynchronize = Some(wrap_hipDeviceSynchronize);
    wrapper.hipMalloc = Some(wrap_hipMalloc);
    wrapper.hipFree = Some(wrap_hipFree);
    wrapper.hipHostMalloc = Some(wrap_hipHostMalloc);
    wrapper.hipHostFree = Some(wrap_hipHostFree);
    wrapper.hipMemcpy = Some(wrap_hipMemcpy);
    wrapper.hipMemcpyAsync = Some(wrap_hipMemcpyAsync);
    wrapper.hipMemset = Some(wrap_hipMemset);
    wrapper.hipMemsetAsync = Some(wrap_hipMemsetAsync);
    wrapper.hipStreamCreate = Some(wrap_hipStreamCreate);
    wrapper.hipStreamCreateWithFlags = Some(wrap_hipStreamCreateWithFlags);
    wrapper.hipStreamDestroy = Some(wrap_hipStreamDestroy);
    wrapper.hipStreamSynchronize = Some(wrap_hipStreamSynchronize);
    wrapper.hipEventCreate = Some(wrap_hipEventCreate);
    wrapper.hipEventCreateWithFlags = Some(wrap_hipEventCreateWithFlags);
    wrapper.hipEventDestroy = Some(wrap_hipEventDestroy);
    wrapper.hipEventRecord = Some(wrap_hipEventRecord);
    wrapper.hipEventSynchronize = Some(wrap_hipEventSynchronize);
    wrapper.hipModuleLaunchKernel = Some(wrap_hipModuleLaunchKernel);
    wrapper.hipLaunchKernel = Some(wrap_hipLaunchKernel);
    wrapper.hipExtModuleLaunchKernel = Some(wrap_hipExtModuleLaunchKernel);
    wrapper.__hipRegisterFatBinary = Some(wrap___hipRegisterFatBinary);
    wrapper.__hipUnregisterFatBinary = Some(wrap___hipUnregisterFatBinary);
    wrapper.__hipRegisterFunction = Some(wrap___hipRegisterFunction);
    wrapper.__hipRegisterVar = Some(wrap___hipRegisterVar);

    let wrapper_ptr = Box::into_raw(Box::new(wrapper));

    let st = State {
        real: real_functions,
        wrapper: wrapper_ptr,
        trace_file: file,
        trace_level,
        trace_sync,
        trace_dump,
        trace_dump_max,
        kernel_filter,
        kernel_count_limit,
        kernel_count: 0,
        kernel_full_dump_list,
        owns_file,
        buffers: BufferTable {
            buffers: vec![
                TrackedBuffer { ptr: core::ptr::null_mut(), size: 0, btype: BufferType::Device, in_use: false, alloc_id: 0 };
                MAX_TRACKED_BUFFERS
            ],
            count: 0,
            next_alloc_id: 1,
        },
        kernels: Vec::with_capacity(256),
    };

    // Emit the init banner lines (matching the C order/format), then store.
    {
        let mut guard = STATE.lock().unwrap();
        *guard = Some(st);
        let st = guard.as_ref().unwrap();
        if let Some(list) = &st.kernel_full_dump_list {
            trace_msg(st, 1, &format!("Full dump enabled for kernels (len={}): {}", list.len(), list));
        }
        trace_msg(st, 1, "HIP Buffer Tracer initialized");
        trace_msg(st, 1, &format!(
            "  trace_level={}, trace_sync={}, trace_dump={}, dump_max={}",
            st.trace_level, st.trace_sync as i32, st.trace_dump, st.trace_dump_max));
        if let Some(f) = &st.kernel_filter {
            if !f.is_empty() {
                trace_msg(st, 1, &format!("  kernel_filter={}", f));
            }
        }
        if st.kernel_count_limit > 0 {
            trace_msg(st, 1, &format!("  kernel_count_limit={}", st.kernel_count_limit));
        }
    }

    wrapper_ptr
}

#[no_mangle]
pub unsafe extern "C" fn hip_interceptor_shutdown() {
    let mut guard = STATE.lock().unwrap();
    if let Some(st) = guard.as_mut() {
        trace_msg(st, 1, "HIP Buffer Tracer shutting down");
        trace_msg(st, 1, &format!("  Total kernels traced: {}", st.kernel_count));
        trace_msg(st, 1, &format!("  Total buffers tracked: {}", st.buffers.count));
        if st.owns_file && !st.trace_file.is_null() {
            libc::fclose(st.trace_file);
            st.trace_file = core::ptr::null_mut();
        }
        // Free the leaked wrapper table.
        if !st.wrapper.is_null() {
            drop(Box::from_raw(st.wrapper));
            st.wrapper = core::ptr::null_mut();
        }
    }
}
