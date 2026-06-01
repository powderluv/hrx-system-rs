//! Rust port of `libhrx/src/passthrough/interceptors/passthrough_interceptor.c`.
//!
//! The no-op interceptor: `hip_interceptor_init` returns NULL, so the
//! passthrough uses the real function table directly (every HIP call forwards
//! straight to the backend). A minimal template for custom interceptors.

use hip_function_table::HipFunctionTable;

/// Return NULL to use the real functions directly. To intercept, build your own
/// wrapper table and return it instead (see hip-logging / hip-buffer-tracer).
#[no_mangle]
pub extern "C" fn hip_interceptor_init(
    _real_functions: *mut HipFunctionTable,
) -> *mut HipFunctionTable {
    core::ptr::null_mut()
}

/// Optional shutdown — nothing to clean up for the passthrough.
#[no_mangle]
pub extern "C" fn hip_interceptor_shutdown() {}
