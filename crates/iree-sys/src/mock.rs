//! In-memory mock of the IREE objects' refcount lifecycle, compiled **only under
//! Miri** (`cfg(miri)`). Under Miri the real `iree_*_retain`/`release` externs are
//! replaced (see the `#[cfg(miri)]` shims in `init`/`fem`) by calls into the
//! `retain`/`release` here, and tests mint handles with [`new_handle`].
//!
//! Each handle is a real heap-allocated refcount node, so **Miri's own allocator
//! tracking is the oracle**: a leak (a handle never released to zero) is reported
//! by Miri's leak check, and a release-below-zero frees the node twice → Miri
//! reports the double-free / use-after-free. This is the "in-memory fake" the
//! safety plan calls for — a tiny Rust stand-in for the IREE object lifecycle, not
//! a production runtime (Miri can only check Rust-side memory discipline, so a real
//! Rust IREE would add nothing over this for the ownership/refcount properties we
//! verify).
use core::ffi::c_void;
use core::sync::atomic::{AtomicI32, Ordering};

#[repr(C)]
struct MockHandle {
    count: AtomicI32,
}

/// Mint a fresh mock handle with refcount 1 — the stand-in for an IREE `*_create`.
/// The returned pointer is a real heap allocation Miri tracks.
pub fn new_handle() -> *mut c_void {
    Box::into_raw(Box::new(MockHandle { count: AtomicI32::new(1) })) as *mut c_void
}

/// Add one reference (stand-in for `iree_*_retain`). NULL is a no-op, matching the
/// NULL-safe IREE retains the port relies on.
///
/// # Safety
/// `h`, if non-null, must be a live handle from [`new_handle`].
pub unsafe fn retain(h: *mut c_void) {
    if h.is_null() {
        return;
    }
    // SAFETY: non-null handles come from `new_handle` and are live until released
    // to zero; the caller upholds that.
    unsafe { (*(h as *mut MockHandle)).count.fetch_add(1, Ordering::Relaxed) };
}

/// Drop one reference (stand-in for `iree_*_release`); frees the node at zero. NULL
/// is a no-op. A release below zero frees the node a second time — which Miri
/// reports as a double-free, catching any wrapper that releases too often.
///
/// # Safety
/// `h`, if non-null, must be a live handle from [`new_handle`].
pub unsafe fn release(h: *mut c_void) {
    if h.is_null() {
        return;
    }
    // SAFETY: as above; on the last reference we reclaim the Box and drop it.
    if unsafe { (*(h as *mut MockHandle)).count.fetch_sub(1, Ordering::AcqRel) } == 1 {
        drop(unsafe { Box::from_raw(h as *mut MockHandle) });
    }
}
