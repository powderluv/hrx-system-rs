//! `#[repr(C)]` mirror of `libhrx/src/passthrough/hip_function_table.h`.
//!
//! This is the single source of truth for the passthrough interceptor ABI in
//! the Rust port. The struct layout MUST match the C header exactly so that a
//! Rust interceptor can be loaded by the C `libhip_intercept.so` (and vice
//! versa). A Rust interceptor receives `*mut HipFunctionTable` from
//! `hip_interceptor_init`, copies it, overrides selected entries with its own
//! `extern "C"` wrappers, and returns a pointer to the wrapper table.
#![allow(non_snake_case, non_camel_case_types)]

use core::ffi::{c_char, c_float, c_int, c_uint, c_void};

// HIP scalar / handle types (see header lines 21-113).
pub type hipError_t = c_int;
pub type hipStream_t = *mut c_void;
pub type hipEvent_t = *mut c_void;
pub type hipModule_t = *mut c_void;
pub type hipFunction_t = *mut c_void;
pub type hipDeviceptr_t = *mut c_void;
pub type hipDeviceProp_t = c_void;
pub type hipMemcpyKind = c_int;
pub type hipDeviceAttribute_t = c_uint;

#[repr(C)]
#[derive(Clone, Copy)]
pub struct dim3 {
    pub x: c_uint,
    pub y: c_uint,
    pub z: c_uint,
}

// Function-pointer typedefs (header lines 123-253). Nullable -> Option<fn>.
pub type pfn_hipInit = Option<unsafe extern "C" fn(c_uint) -> hipError_t>;
pub type pfn_hipDriverGetVersion = Option<unsafe extern "C" fn(*mut c_int) -> hipError_t>;
pub type pfn_hipRuntimeGetVersion = Option<unsafe extern "C" fn(*mut c_int) -> hipError_t>;
pub type pfn_hipGetDevice = Option<unsafe extern "C" fn(*mut c_int) -> hipError_t>;
pub type pfn_hipGetDeviceCount = Option<unsafe extern "C" fn(*mut c_int) -> hipError_t>;
pub type pfn_hipSetDevice = Option<unsafe extern "C" fn(c_int) -> hipError_t>;
pub type pfn_hipDeviceReset = Option<unsafe extern "C" fn() -> hipError_t>;
pub type pfn_hipDeviceSynchronize = Option<unsafe extern "C" fn() -> hipError_t>;
pub type pfn_hipGetDeviceProperties =
    Option<unsafe extern "C" fn(*mut hipDeviceProp_t, c_int) -> hipError_t>;
pub type pfn_hipDeviceGetAttribute =
    Option<unsafe extern "C" fn(*mut c_int, hipDeviceAttribute_t, c_int) -> hipError_t>;
pub type pfn_hipDeviceGetName =
    Option<unsafe extern "C" fn(*mut c_char, c_int, c_int) -> hipError_t>;

pub type pfn_hipMalloc = Option<unsafe extern "C" fn(*mut *mut c_void, usize) -> hipError_t>;
pub type pfn_hipFree = Option<unsafe extern "C" fn(*mut c_void) -> hipError_t>;
pub type pfn_hipHostMalloc =
    Option<unsafe extern "C" fn(*mut *mut c_void, usize, c_uint) -> hipError_t>;
pub type pfn_hipHostFree = Option<unsafe extern "C" fn(*mut c_void) -> hipError_t>;
pub type pfn_hipMemGetInfo = Option<unsafe extern "C" fn(*mut usize, *mut usize) -> hipError_t>;

pub type pfn_hipMemcpy =
    Option<unsafe extern "C" fn(*mut c_void, *const c_void, usize, hipMemcpyKind) -> hipError_t>;
pub type pfn_hipMemcpyAsync = Option<
    unsafe extern "C" fn(*mut c_void, *const c_void, usize, hipMemcpyKind, hipStream_t) -> hipError_t,
>;
pub type pfn_hipMemset = Option<unsafe extern "C" fn(*mut c_void, c_int, usize) -> hipError_t>;
pub type pfn_hipMemsetAsync =
    Option<unsafe extern "C" fn(*mut c_void, c_int, usize, hipStream_t) -> hipError_t>;

pub type pfn_hipStreamCreate = Option<unsafe extern "C" fn(*mut hipStream_t) -> hipError_t>;
pub type pfn_hipStreamCreateWithFlags =
    Option<unsafe extern "C" fn(*mut hipStream_t, c_uint) -> hipError_t>;
pub type pfn_hipStreamDestroy = Option<unsafe extern "C" fn(hipStream_t) -> hipError_t>;
pub type pfn_hipStreamSynchronize = Option<unsafe extern "C" fn(hipStream_t) -> hipError_t>;
pub type pfn_hipStreamQuery = Option<unsafe extern "C" fn(hipStream_t) -> hipError_t>;
pub type pfn_hipStreamWaitEvent =
    Option<unsafe extern "C" fn(hipStream_t, hipEvent_t, c_uint) -> hipError_t>;

pub type pfn_hipEventCreate = Option<unsafe extern "C" fn(*mut hipEvent_t) -> hipError_t>;
pub type pfn_hipEventCreateWithFlags =
    Option<unsafe extern "C" fn(*mut hipEvent_t, c_uint) -> hipError_t>;
pub type pfn_hipEventDestroy = Option<unsafe extern "C" fn(hipEvent_t) -> hipError_t>;
pub type pfn_hipEventRecord = Option<unsafe extern "C" fn(hipEvent_t, hipStream_t) -> hipError_t>;
pub type pfn_hipEventSynchronize = Option<unsafe extern "C" fn(hipEvent_t) -> hipError_t>;
pub type pfn_hipEventQuery = Option<unsafe extern "C" fn(hipEvent_t) -> hipError_t>;
pub type pfn_hipEventElapsedTime =
    Option<unsafe extern "C" fn(*mut c_float, hipEvent_t, hipEvent_t) -> hipError_t>;

pub type pfn_hipModuleLoad =
    Option<unsafe extern "C" fn(*mut hipModule_t, *const c_char) -> hipError_t>;
pub type pfn_hipModuleLoadData =
    Option<unsafe extern "C" fn(*mut hipModule_t, *const c_void) -> hipError_t>;
pub type pfn_hipModuleUnload = Option<unsafe extern "C" fn(hipModule_t) -> hipError_t>;
pub type pfn_hipModuleGetFunction =
    Option<unsafe extern "C" fn(*mut hipFunction_t, hipModule_t, *const c_char) -> hipError_t>;
pub type pfn_hipModuleGetGlobal = Option<
    unsafe extern "C" fn(*mut hipDeviceptr_t, *mut usize, hipModule_t, *const c_char) -> hipError_t,
>;
pub type pfn_hipModuleLaunchKernel = Option<
    unsafe extern "C" fn(
        hipFunction_t,
        c_uint,
        c_uint,
        c_uint,
        c_uint,
        c_uint,
        c_uint,
        c_uint,
        hipStream_t,
        *mut *mut c_void,
        *mut *mut c_void,
    ) -> hipError_t,
>;
pub type pfn_hipLaunchKernel = Option<
    unsafe extern "C" fn(*const c_void, dim3, dim3, *mut *mut c_void, usize, hipStream_t) -> hipError_t,
>;
pub type pfn_hipExtModuleLaunchKernel = Option<
    unsafe extern "C" fn(
        hipFunction_t,
        c_uint,
        c_uint,
        c_uint,
        c_uint,
        c_uint,
        c_uint,
        usize,
        hipStream_t,
        *mut *mut c_void,
        *mut *mut c_void,
        hipEvent_t,
        hipEvent_t,
        c_uint,
    ) -> hipError_t,
>;

pub type pfn___hipRegisterFatBinary = Option<unsafe extern "C" fn(*const c_void) -> *mut *mut c_void>;
pub type pfn___hipUnregisterFatBinary = Option<unsafe extern "C" fn(*mut *mut c_void)>;
pub type pfn___hipRegisterFunction = Option<
    unsafe extern "C" fn(
        *mut *mut c_void,
        *const c_char,
        *mut c_char,
        *const c_char,
        c_int,
        *mut c_void,
        *mut c_void,
        *mut dim3,
        *mut dim3,
        *mut c_int,
    ),
>;
pub type pfn___hipRegisterVar = Option<
    unsafe extern "C" fn(
        *mut *mut c_void,
        *mut c_char,
        *mut c_char,
        *const c_char,
        c_int,
        usize,
        c_int,
        c_int,
    ),
>;

pub type pfn_hipGetErrorString = Option<unsafe extern "C" fn(hipError_t) -> *const c_char>;
pub type pfn_hipGetErrorName = Option<unsafe extern "C" fn(hipError_t) -> *const c_char>;
pub type pfn_hipGetLastError = Option<unsafe extern "C" fn() -> hipError_t>;
pub type pfn_hipPeekAtLastError = Option<unsafe extern "C" fn() -> hipError_t>;

/// `struct hip_function_table_t` (header lines 259-328). Field order is ABI.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct HipFunctionTable {
    pub version: u32,
    pub struct_size: u32,

    // Device Management
    pub hipInit: pfn_hipInit,
    pub hipDriverGetVersion: pfn_hipDriverGetVersion,
    pub hipRuntimeGetVersion: pfn_hipRuntimeGetVersion,
    pub hipGetDevice: pfn_hipGetDevice,
    pub hipGetDeviceCount: pfn_hipGetDeviceCount,
    pub hipSetDevice: pfn_hipSetDevice,
    pub hipDeviceReset: pfn_hipDeviceReset,
    pub hipDeviceSynchronize: pfn_hipDeviceSynchronize,
    pub hipGetDeviceProperties: pfn_hipGetDeviceProperties,
    pub hipDeviceGetAttribute: pfn_hipDeviceGetAttribute,
    pub hipDeviceGetName: pfn_hipDeviceGetName,

    // Memory Management
    pub hipMalloc: pfn_hipMalloc,
    pub hipFree: pfn_hipFree,
    pub hipHostMalloc: pfn_hipHostMalloc,
    pub hipHostFree: pfn_hipHostFree,
    pub hipMemGetInfo: pfn_hipMemGetInfo,

    // Memory Copy
    pub hipMemcpy: pfn_hipMemcpy,
    pub hipMemcpyAsync: pfn_hipMemcpyAsync,
    pub hipMemset: pfn_hipMemset,
    pub hipMemsetAsync: pfn_hipMemsetAsync,

    // Stream Management
    pub hipStreamCreate: pfn_hipStreamCreate,
    pub hipStreamCreateWithFlags: pfn_hipStreamCreateWithFlags,
    pub hipStreamDestroy: pfn_hipStreamDestroy,
    pub hipStreamSynchronize: pfn_hipStreamSynchronize,
    pub hipStreamQuery: pfn_hipStreamQuery,
    pub hipStreamWaitEvent: pfn_hipStreamWaitEvent,

    // Event Management
    pub hipEventCreate: pfn_hipEventCreate,
    pub hipEventCreateWithFlags: pfn_hipEventCreateWithFlags,
    pub hipEventDestroy: pfn_hipEventDestroy,
    pub hipEventRecord: pfn_hipEventRecord,
    pub hipEventSynchronize: pfn_hipEventSynchronize,
    pub hipEventQuery: pfn_hipEventQuery,
    pub hipEventElapsedTime: pfn_hipEventElapsedTime,

    // Module Management
    pub hipModuleLoad: pfn_hipModuleLoad,
    pub hipModuleLoadData: pfn_hipModuleLoadData,
    pub hipModuleUnload: pfn_hipModuleUnload,
    pub hipModuleGetFunction: pfn_hipModuleGetFunction,
    pub hipModuleGetGlobal: pfn_hipModuleGetGlobal,
    pub hipModuleLaunchKernel: pfn_hipModuleLaunchKernel,
    pub hipLaunchKernel: pfn_hipLaunchKernel,
    pub hipExtModuleLaunchKernel: pfn_hipExtModuleLaunchKernel,

    // Fat Binary Registration
    pub __hipRegisterFatBinary: pfn___hipRegisterFatBinary,
    pub __hipUnregisterFatBinary: pfn___hipUnregisterFatBinary,
    pub __hipRegisterFunction: pfn___hipRegisterFunction,
    pub __hipRegisterVar: pfn___hipRegisterVar,

    // Error Handling
    pub hipGetErrorString: pfn_hipGetErrorString,
    pub hipGetErrorName: pfn_hipGetErrorName,
    pub hipGetLastError: pfn_hipGetLastError,
    pub hipPeekAtLastError: pfn_hipPeekAtLastError,
}

/// Interceptor entry points an interceptor `.so` may export.
pub type pfn_hip_interceptor_init =
    Option<unsafe extern "C" fn(*mut HipFunctionTable) -> *mut HipFunctionTable>;
pub type pfn_hip_interceptor_shutdown = Option<unsafe extern "C" fn()>;

/// HIP memcpy-kind names, matching `memcpy_kind_name` in hip_logging.c.
pub fn memcpy_kind_name(kind: hipMemcpyKind) -> &'static str {
    match kind {
        0 => "H2H",
        1 => "H2D",
        2 => "D2H",
        3 => "D2D",
        4 => "Default",
        _ => "Unknown",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    // The C struct has 2 u32 header words + 49 pointer-sized function slots
    // (device 11, memory 5, memcpy 4, stream 6, event 7, module 8, fatbin 4,
    // error 4). On LP64: 8 + 49*8 = 400 bytes. Guards against field drift.
    #[test]
    fn table_size_matches_c_abi() {
        assert_eq!(core::mem::size_of::<HipFunctionTable>(), 8 + 49 * 8);
        assert_eq!(core::mem::align_of::<HipFunctionTable>(), 8);
    }
}
