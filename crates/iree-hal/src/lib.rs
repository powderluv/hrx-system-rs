//! Safe RAII wrappers over IREE HAL / runtime objects.
//!
//! Each wrapper owns exactly one reference to the underlying IREE object:
//! - `Drop` releases it (`iree_hal_*_release`),
//! - `Clone` retains it (`iree_hal_*_retain`),
//! - constructors return the wrapper holding the freshly-created +1 reference.
//!
//! This is the single place the IREE retain/release FFI lives, so the rest of
//! the port calls safe methods instead of scattering `unsafe` FFI through the
//! business logic (the safety assessment's "FFI shape" point). Each wrapper is
//! `#[repr(transparent)]` over a `NonNull` raw pointer and every method is a thin
//! `#[inline]` FFI call, so this compiles to the same machine code as the direct
//! calls it replaces — no added indirection, no perf cost.
//!
//! This crate is grown one object type at a time as each `hrx-libhrx` module is
//! migrated off raw pointers; today it covers `iree_hal_fence_t`.
#![forbid(unsafe_op_in_unsafe_fn)]

use core::ptr::NonNull;
use iree_sys as iree;
use iree_sys::fem;
use iree_sys::init as ireei;

/// Owned reference to an `iree_hal_fence_t`. `Drop` releases, `Clone` retains.
#[repr(transparent)]
pub struct HalFence(NonNull<fem::iree_hal_fence_t>);

impl HalFence {
    /// Wrap a freshly-created or otherwise-owned fence pointer, moving the
    /// caller's +1 reference into the wrapper. Returns `None` if `ptr` is null.
    ///
    /// # Safety
    /// `ptr` must be a valid `iree_hal_fence_t*` carrying a reference the caller
    /// is transferring (not borrowing).
    #[inline]
    pub unsafe fn from_owned(ptr: *mut fem::iree_hal_fence_t) -> Option<Self> {
        NonNull::new(ptr).map(Self)
    }

    /// Borrow the raw pointer for an FFI call that does not take ownership.
    #[inline]
    pub fn as_ptr(&self) -> *mut fem::iree_hal_fence_t {
        self.0.as_ptr()
    }

    #[inline]
    pub fn insert(
        &self,
        semaphore: *mut ireei::iree_hal_semaphore_t,
        value: u64,
    ) -> iree::iree_status_t {
        // SAFETY: self.0 is a live fence; `semaphore` is validated by the caller.
        unsafe { fem::iree_hal_fence_insert(self.0.as_ptr(), semaphore, value) }
    }

    #[inline]
    pub fn extend(&self, from: &HalFence) -> iree::iree_status_t {
        // SAFETY: both fences are live for the duration of the call.
        unsafe { fem::iree_hal_fence_extend(self.0.as_ptr(), from.0.as_ptr()) }
    }

    #[inline]
    pub fn signal(&self) -> iree::iree_status_t {
        // SAFETY: self.0 is a live fence.
        unsafe { fem::iree_hal_fence_signal(self.0.as_ptr()) }
    }

    #[inline]
    pub fn wait(&self, timeout: ireei::iree_timeout_t) -> iree::iree_status_t {
        // SAFETY: self.0 is a live fence.
        unsafe { fem::iree_hal_fence_wait(self.0.as_ptr(), timeout, 0) }
    }
}

impl Clone for HalFence {
    #[inline]
    fn clone(&self) -> Self {
        // SAFETY: self.0 is a live fence; retain bumps its refcount.
        unsafe { fem::iree_hal_fence_retain(self.0.as_ptr()) };
        Self(self.0)
    }
}

impl Drop for HalFence {
    #[inline]
    fn drop(&mut self) {
        // SAFETY: self owns one reference; release returns it exactly once.
        unsafe { fem::iree_hal_fence_release(self.0.as_ptr()) };
    }
}

/// Create a fence with the given semaphore capacity.
#[inline]
pub fn fence_create(capacity: usize) -> Result<HalFence, iree::iree_status_t> {
    let mut hal: *mut fem::iree_hal_fence_t = core::ptr::null_mut();
    // SAFETY: out-pointer is valid; allocator_system() is a valid allocator.
    let s = unsafe { fem::iree_hal_fence_create(capacity, iree::allocator_system(), &mut hal) };
    if iree::status_is_ok(s) {
        // SAFETY: on OK, `hal` is a freshly-created owned fence.
        Ok(unsafe { HalFence::from_owned(hal) }.expect("OK status with null fence"))
    } else {
        Err(s)
    }
}

/// Create a fence pre-inserted at `(semaphore, value)`.
///
/// # Safety
/// `semaphore` must be a valid `iree_hal_semaphore_t*`.
#[inline]
pub unsafe fn fence_create_at(
    semaphore: *mut ireei::iree_hal_semaphore_t,
    value: u64,
) -> Result<HalFence, iree::iree_status_t> {
    let mut hal: *mut fem::iree_hal_fence_t = core::ptr::null_mut();
    // SAFETY: out-pointer is valid; `semaphore` is the caller's responsibility.
    let s = unsafe {
        fem::iree_hal_fence_create_at(semaphore, value, iree::allocator_system(), &mut hal)
    };
    if iree::status_is_ok(s) {
        // SAFETY: on OK, `hal` is a freshly-created owned fence.
        Ok(unsafe { HalFence::from_owned(hal) }.expect("OK status with null fence"))
    } else {
        Err(s)
    }
}
