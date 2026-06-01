//! Proves the Rust crate statically links the IREE archives and can call into
//! the C runtime: allocate + free via the IREE system allocator.

use core::ffi::c_void;
use iree_sys::*;

#[test]
fn iree_allocator_roundtrip() {
    let sys = allocator_system();
    let mut p: *mut c_void = core::ptr::null_mut();
    let rc = unsafe { iree_allocator_malloc(sys, 4096, &mut p) };
    assert!(status_is_ok(rc), "iree_allocator_malloc failed code={}", status_code(rc));
    assert!(!p.is_null(), "allocation returned null");
    unsafe { iree_allocator_free(sys, p) };
}

#[test]
fn vm_value_layout_matches_c_abi() {
    // iree_vm_value_t is 16 bytes (i32 type + pad + 8-byte 8-aligned union).
    assert_eq!(core::mem::size_of::<iree_vm_value_t>(), 16);
    assert_eq!(core::mem::align_of::<iree_vm_value_t>(), 8);
    let v = iree_vm_value_t::make_i64(-12345);
    assert_eq!(v.as_i64(), -12345);
}
