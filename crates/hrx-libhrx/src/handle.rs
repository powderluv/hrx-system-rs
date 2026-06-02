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

#[cfg(test)]
mod tests {
    //! Miri-runnable verification of the `Arc` handle boundary — the single most
    //! central `unsafe` in the port. These tests are pure Rust (no IREE FFI), so
    //! `cargo +nightly miri test` exercises them directly and checks for
    //! use-after-free, double-free, leaks, and invalid aliasing in the
    //! retain/release/borrow/destructure machinery that every migrated object
    //! relies on. `Probe` counts its own drops through a borrowed `Cell` that
    //! outlives every handle, so a leak (no drop) or double-free (drop twice) is
    //! caught by the assertions and any UB by Miri itself.
    use super::*;
    use core::cell::Cell;

    struct Probe<'a> {
        drops: &'a Cell<u32>,
        tag: u32,
    }
    impl Drop for Probe<'_> {
        fn drop(&mut self) {
            self.drops.set(self.drops.get() + 1);
        }
    }

    #[test]
    fn release_drops_exactly_once() {
        let drops = Cell::new(0);
        let h = into_handle(Probe { drops: &drops, tag: 7 });
        // Borrow before release: the object is live and readable.
        assert_eq!(unsafe { handle_ref(h) }.tag, 7);
        assert_eq!(drops.get(), 0);
        unsafe { handle_release(h) };
        assert_eq!(drops.get(), 1, "last release must drop exactly once");
    }

    #[test]
    fn retain_then_two_releases_drop_once_at_zero() {
        let drops = Cell::new(0);
        let h = into_handle(Probe { drops: &drops, tag: 1 });
        unsafe { handle_retain(h) }; // count: 2
        unsafe { handle_release(h) }; // count: 1 — must NOT drop yet
        assert_eq!(drops.get(), 0, "non-final release must not drop");
        unsafe { handle_release(h) }; // count: 0 — drop now
        assert_eq!(drops.get(), 1, "object dropped once when count hits zero");
    }

    #[test]
    fn handle_ref_survives_extra_references() {
        let drops = Cell::new(0);
        let h = into_handle(Probe { drops: &drops, tag: 42 });
        unsafe { handle_retain(h) };
        // A borrow taken while two references exist stays valid across a release
        // that does not drop the last reference.
        let r = unsafe { handle_ref(h) };
        unsafe { handle_release(h) };
        assert_eq!(r.tag, 42);
        unsafe { handle_release(h) };
        assert_eq!(drops.get(), 1);
    }

    #[test]
    fn into_inner_takes_object_on_last_reference() {
        let drops = Cell::new(0);
        let h = into_handle(Probe { drops: &drops, tag: 9 });
        let inner = unsafe { into_inner_handle(h) }.expect("sole reference yields Some");
        assert_eq!(inner.tag, 9);
        assert_eq!(drops.get(), 0, "into_inner moves the object out, not drops it");
        drop(inner);
        assert_eq!(drops.get(), 1, "moved-out object drops exactly once");
    }

    #[test]
    fn into_inner_returns_none_with_outstanding_reference() {
        let drops = Cell::new(0);
        let h = into_handle(Probe { drops: &drops, tag: 3 });
        unsafe { handle_retain(h) }; // count: 2
        // into_inner consumes one reference; with another outstanding it yields
        // None and must not drop the object.
        assert!(unsafe { into_inner_handle(h) }.is_none());
        assert_eq!(drops.get(), 0);
        // The surviving reference still owns the object; releasing it drops once.
        unsafe { handle_release(h) };
        assert_eq!(drops.get(), 1);
    }

    #[test]
    fn cyclic_handle_receives_its_own_address() {
        let drops = Cell::new(0);
        // `into_handle_cyclic` must hand `build` the address the handle will have.
        let captured: Cell<*const Probe> = Cell::new(core::ptr::null());
        let h = into_handle_cyclic(|self_ptr| {
            captured.set(self_ptr);
            Probe { drops: &drops, tag: 5 }
        });
        assert_eq!(captured.get(), h as *const Probe, "build sees the final address");
        assert_eq!(unsafe { handle_ref(h) }.tag, 5);
        unsafe { handle_release(h) };
        assert_eq!(drops.get(), 1);
    }
}
