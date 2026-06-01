//! Proves the Rust crate statically links the IREE archives and can call into
//! the C runtime: allocate + free via the IREE system allocator.

use core::ffi::c_void;
use iree_sys::*;

#[test]
fn iree_allocator_roundtrip() {
    let sys = allocator_system();
    let mut p: *mut c_void = core::ptr::null_mut();
    let rc = unsafe { iree_allocator_malloc(sys, 4096, &mut p) };
    assert_eq!(rc, 0, "iree_allocator_malloc returned {rc}");
    assert!(!p.is_null(), "allocation returned null");
    unsafe { iree_allocator_free(sys, p) };
}
