//! The single audited boundary between the public C ABI's opaque handles and the
//! Rust-owned objects behind them.
//!
//! A handle is the data pointer of a `std::sync::Arc<T>` (so the C ABI still sees
//! a plain `*mut hrx_X_s`, and field access is a single deref — no extra
//! indirection). `retain`/`release` become `Arc` refcount operations, which use
//! the same atomic increment/decrement as the old hand-rolled `AtomicI32`
//! refcount, and the object's `Drop` runs the IREE/parent teardown exactly once
//! on the last release. This replaces manual `libc::calloc`/`free` +
//! `iree_*_retain`/`release` accounting with compiler-enforced ownership, while
//! keeping the C-visible behavior (and performance) identical.
//!
//! Every `unsafe` here trusts a caller-provided handle — which is inherent to a C
//! ABI (C callers can always pass garbage). It is confined to this module so the
//! migrated business logic stays safe Rust.
use std::sync::Arc;

/// Move `obj` into a new ref-counted allocation and return an opaque handle
/// (the `Arc` data pointer). The returned handle owns one reference.
#[inline]
pub(crate) fn into_handle<T>(obj: T) -> *mut T {
    Arc::into_raw(Arc::new(obj)) as *mut T
}

/// Like [`into_handle`], but for an object that must store its own (future)
/// handle address — e.g. the device's inline allocator back-pointer. `build`
/// receives the data pointer the handle will have and returns the object; the
/// self-reference is established during construction via `Arc::new_cyclic`, so
/// no raw write into shared `Arc` data is needed.
#[inline]
pub(crate) fn into_handle_cyclic<T>(build: impl FnOnce(*const T) -> T) -> *mut T {
    Arc::into_raw(Arc::new_cyclic(|weak| build(weak.as_ptr()))) as *mut T
}

/// Add one reference to a live handle (`hrx_*_retain`).
///
/// # Safety
/// `h` must be a non-null handle previously returned by [`into_handle`] and not
/// yet released to zero.
#[inline]
pub(crate) unsafe fn handle_retain<T>(h: *const T) {
    // SAFETY: caller guarantees `h` is a live Arc data pointer.
    unsafe { Arc::increment_strong_count(h) };
}

/// Drop one reference (`hrx_*_release`); runs `T::drop` on the last release.
///
/// # Safety
/// `h` must be a non-null live handle previously returned by [`into_handle`].
#[inline]
pub(crate) unsafe fn handle_release<T>(h: *const T) {
    // SAFETY: caller guarantees `h` is a live Arc data pointer.
    unsafe { Arc::decrement_strong_count(h) };
}

/// Borrow the object behind a live handle.
///
/// # Safety
/// `h` must be a non-null live handle; the returned borrow must not outlive a
/// concurrent release that drops the last reference.
#[inline]
pub(crate) unsafe fn handle_ref<'a, T>(h: *const T) -> &'a T {
    // SAFETY: caller guarantees `h` points at a live `T`.
    unsafe { &*h }
}

/// Reclaim the caller's owning reference and move the inner object out of its
/// `Arc`, returning `Some(T)` when this was the last reference. Returns `None`
/// (and drops the reclaimed reference) if other references are still outstanding.
///
/// Used by destructive single-owner teardown that must take the object apart by
/// field — e.g. the virtual-memory release, which hands the HAL buffer to a
/// different release function than the normal `Drop` would call.
///
/// # Safety
/// `h` must be a non-null live handle previously returned by [`into_handle`], and
/// the caller is transferring its owning reference (the handle must not be used
/// again).
#[inline]
pub(crate) unsafe fn into_inner_handle<T>(h: *const T) -> Option<T> {
    // SAFETY: caller transfers one owning reference; reconstruct the Arc from it.
    let arc = unsafe { Arc::from_raw(h) };
    Arc::into_inner(arc)
}
