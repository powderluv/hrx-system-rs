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
//! migrated off raw pointers; today it covers `iree_hal_fence_t`,
//! `iree_hal_semaphore_t`, `iree_hal_buffer_view_t`, `iree_hal_device_t`,
//! `iree_hal_device_group_t`, `iree_hal_allocator_t`, `iree_hal_buffer_t`, and
//! `iree_hal_pool_t`.
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

/// Owned reference to an `iree_hal_semaphore_t`. `Drop` releases, `Clone` retains.
#[repr(transparent)]
pub struct HalSemaphore(NonNull<ireei::iree_hal_semaphore_t>);

impl HalSemaphore {
    /// # Safety
    /// `ptr` must be a valid owned `iree_hal_semaphore_t*` (reference transferred in).
    #[inline]
    pub unsafe fn from_owned(ptr: *mut ireei::iree_hal_semaphore_t) -> Option<Self> {
        NonNull::new(ptr).map(Self)
    }

    #[inline]
    pub fn as_ptr(&self) -> *mut ireei::iree_hal_semaphore_t {
        self.0.as_ptr()
    }

    /// Query the current payload into `out_value`, returning the status. Mirrors
    /// the C path (IREE writes `out_value` regardless of the returned status, so
    /// failure-state values propagate identically).
    #[inline]
    pub fn query_into(&self, out_value: &mut u64) -> iree::iree_status_t {
        // SAFETY: self.0 is live; out_value is a valid &mut.
        unsafe { ireei::iree_hal_semaphore_query(self.0.as_ptr(), out_value) }
    }

    #[inline]
    pub fn wait(&self, value: u64, timeout: ireei::iree_timeout_t) -> iree::iree_status_t {
        // SAFETY: self.0 is live.
        unsafe { ireei::iree_hal_semaphore_wait(self.0.as_ptr(), value, timeout, 0) }
    }

    #[inline]
    pub fn signal(&self, value: u64) -> iree::iree_status_t {
        // SAFETY: self.0 is live; a null frontier means "no frontier".
        unsafe { ireei::iree_hal_semaphore_signal(self.0.as_ptr(), value, core::ptr::null_mut()) }
    }
}

impl Clone for HalSemaphore {
    #[inline]
    fn clone(&self) -> Self {
        // SAFETY: self.0 is live; retain bumps its refcount.
        unsafe { ireei::iree_hal_semaphore_retain(self.0.as_ptr()) };
        Self(self.0)
    }
}

impl Drop for HalSemaphore {
    #[inline]
    fn drop(&mut self) {
        // SAFETY: self owns one reference; release returns it once.
        unsafe { ireei::iree_hal_semaphore_release(self.0.as_ptr()) };
    }
}

/// Create a timeline semaphore on `device`.
///
/// # Safety
/// `device` must be a valid `iree_hal_device_t*`.
#[inline]
pub unsafe fn semaphore_create(
    device: *mut iree::iree_hal_device_t,
    queue_affinity: u64,
    initial_value: u64,
    flags: u32,
) -> Result<HalSemaphore, iree::iree_status_t> {
    let mut hal: *mut ireei::iree_hal_semaphore_t = core::ptr::null_mut();
    // SAFETY: out-pointer is valid; `device` is the caller's responsibility.
    let s = unsafe {
        ireei::iree_hal_semaphore_create(device, queue_affinity, initial_value, flags, &mut hal)
    };
    if iree::status_is_ok(s) {
        // SAFETY: on OK, `hal` is a freshly-created owned semaphore.
        Ok(unsafe { HalSemaphore::from_owned(hal) }.expect("OK status with null semaphore"))
    } else {
        Err(s)
    }
}

/// Owned reference to an `iree_hal_buffer_view_t`. `Drop` releases, `Clone` retains.
#[repr(transparent)]
pub struct HalBufferView(NonNull<fem::iree_hal_buffer_view_t>);

impl HalBufferView {
    /// # Safety
    /// `ptr` must be a valid owned `iree_hal_buffer_view_t*`.
    #[inline]
    pub unsafe fn from_owned(ptr: *mut fem::iree_hal_buffer_view_t) -> Option<Self> {
        NonNull::new(ptr).map(Self)
    }

    #[inline]
    pub fn as_ptr(&self) -> *mut fem::iree_hal_buffer_view_t {
        self.0.as_ptr()
    }

    #[inline]
    pub fn shape_rank(&self) -> usize {
        // SAFETY: self.0 is live.
        unsafe { fem::iree_hal_buffer_view_shape_rank(self.0.as_ptr()) }
    }

    #[inline]
    pub fn shape_dim(&self, index: usize) -> u64 {
        // SAFETY: self.0 is live; bounds are the caller's responsibility.
        unsafe { fem::iree_hal_buffer_view_shape_dim(self.0.as_ptr(), index) }
    }
}

impl Clone for HalBufferView {
    #[inline]
    fn clone(&self) -> Self {
        // SAFETY: self.0 is live.
        unsafe { fem::iree_hal_buffer_view_retain(self.0.as_ptr()) };
        Self(self.0)
    }
}

impl Drop for HalBufferView {
    #[inline]
    fn drop(&mut self) {
        // SAFETY: self owns one reference.
        unsafe { fem::iree_hal_buffer_view_release(self.0.as_ptr()) };
    }
}

/// Create a buffer view over `buffer` with the given (already-validated) shape.
///
/// # Safety
/// `buffer` must be a valid `iree_hal_buffer_t*` and `shape` must point to
/// `shape_rank` `u64`s.
#[inline]
pub unsafe fn buffer_view_create(
    buffer: *mut ireei::iree_hal_buffer_t,
    shape_rank: usize,
    shape: *const u64,
    element_type: u32,
    encoding_type: u32,
) -> Result<HalBufferView, iree::iree_status_t> {
    let mut hal: *mut fem::iree_hal_buffer_view_t = core::ptr::null_mut();
    // SAFETY: out-pointer is valid; buffer/shape are the caller's responsibility.
    let s = unsafe {
        fem::iree_hal_buffer_view_create(
            buffer,
            shape_rank,
            shape,
            element_type,
            encoding_type,
            iree::allocator_system(),
            &mut hal,
        )
    };
    if iree::status_is_ok(s) {
        // SAFETY: on OK, `hal` is a freshly-created owned buffer view.
        Ok(unsafe { HalBufferView::from_owned(hal) }.expect("OK status with null buffer_view"))
    } else {
        Err(s)
    }
}

/// Owned reference to an `iree_hal_device_t`. Move-only: `Drop` releases the one
/// reference the device object holds for its lifetime. (The port no longer does
/// per-retain HAL device retain/release — the hrx device refcount is an `Arc`.)
#[repr(transparent)]
pub struct HalDevice(NonNull<iree::iree_hal_device_t>);

impl HalDevice {
    /// # Safety
    /// `ptr` must be a valid owned `iree_hal_device_t*`.
    #[inline]
    pub unsafe fn from_owned(ptr: *mut iree::iree_hal_device_t) -> Option<Self> {
        NonNull::new(ptr).map(Self)
    }
    #[inline]
    pub fn as_ptr(&self) -> *mut iree::iree_hal_device_t {
        self.0.as_ptr()
    }
}

impl Drop for HalDevice {
    #[inline]
    fn drop(&mut self) {
        // SAFETY: self owns one reference; release it once.
        unsafe { ireei::iree_hal_device_release(self.0.as_ptr()) };
    }
}

/// Owned reference to an `iree_hal_device_group_t`. Move-only.
#[repr(transparent)]
pub struct HalDeviceGroup(NonNull<iree::iree_hal_device_group_t>);

impl HalDeviceGroup {
    /// # Safety
    /// `ptr` must be a valid owned `iree_hal_device_group_t*`.
    #[inline]
    pub unsafe fn from_owned(ptr: *mut iree::iree_hal_device_group_t) -> Option<Self> {
        NonNull::new(ptr).map(Self)
    }
    #[inline]
    pub fn as_ptr(&self) -> *mut iree::iree_hal_device_group_t {
        self.0.as_ptr()
    }
}

impl Drop for HalDeviceGroup {
    #[inline]
    fn drop(&mut self) {
        // SAFETY: self owns one reference; release it once.
        unsafe { ireei::iree_hal_device_group_release(self.0.as_ptr()) };
    }
}

/// Owned reference to an `iree_hal_allocator_t`. Move-only: `Drop` releases the
/// one reference the device holds. (The public `hrx_allocator_retain`/`release`
/// fans out a separate, balanced `iree_hal_allocator_retain`/`release` on the raw
/// pointer via `as_ptr` — that transient pairing is not owned by this wrapper.)
#[repr(transparent)]
pub struct HalAllocator(NonNull<iree::iree_hal_allocator_t>);

impl HalAllocator {
    /// # Safety
    /// `ptr` must be a valid owned `iree_hal_allocator_t*`.
    #[inline]
    pub unsafe fn from_owned(ptr: *mut iree::iree_hal_allocator_t) -> Option<Self> {
        NonNull::new(ptr).map(Self)
    }
    #[inline]
    pub fn as_ptr(&self) -> *mut iree::iree_hal_allocator_t {
        self.0.as_ptr()
    }
}

impl Drop for HalAllocator {
    #[inline]
    fn drop(&mut self) {
        // SAFETY: self owns one reference; release it once.
        unsafe { ireei::iree_hal_allocator_release(self.0.as_ptr()) };
    }
}

/// Owned reference to an `iree_hal_buffer_t`. Move-only: the hrx buffer object
/// holds one reference for its lifetime and releases it once on drop (the
/// per-retain HAL buffer accounting moved to the `Arc` refcount). `into_raw` hands
/// the pointer back *without* releasing — used by the virtual-memory release path,
/// which consumes the buffer via `iree_hal_allocator_virtual_memory_release`
/// instead of `iree_hal_buffer_release`.
#[repr(transparent)]
pub struct HalBuffer(NonNull<ireei::iree_hal_buffer_t>);

impl HalBuffer {
    /// # Safety
    /// `ptr` must be a valid owned `iree_hal_buffer_t*`.
    #[inline]
    pub unsafe fn from_owned(ptr: *mut ireei::iree_hal_buffer_t) -> Option<Self> {
        NonNull::new(ptr).map(Self)
    }
    #[inline]
    pub fn as_ptr(&self) -> *mut ireei::iree_hal_buffer_t {
        self.0.as_ptr()
    }
    /// Consume the wrapper and return the raw pointer *without* releasing the
    /// reference; the caller takes ownership of the +1.
    #[inline]
    pub fn into_raw(self) -> *mut ireei::iree_hal_buffer_t {
        let p = self.0.as_ptr();
        core::mem::forget(self);
        p
    }
}

impl Drop for HalBuffer {
    #[inline]
    fn drop(&mut self) {
        // SAFETY: self owns one reference; release it once.
        unsafe { ireei::iree_hal_buffer_release(self.0.as_ptr()) };
    }
}

/// Owned reference to an `iree_hal_pool_t` (the per-allocation transient pool on
/// the stream-alloca path). Move-only: `Drop` releases the one reference. Buffers
/// not allocated through a pool simply hold `None`, which matches the C code's
/// NULL-safe `iree_hal_pool_release(NULL)` (a no-op) on those paths.
#[repr(transparent)]
pub struct HalPool(NonNull<iree::iree_hal_pool_t>);

impl HalPool {
    /// Wrap a pool pointer, returning `None` for null (the off-pool paths).
    ///
    /// # Safety
    /// `ptr`, if non-null, must be a valid owned `iree_hal_pool_t*`.
    #[inline]
    pub unsafe fn from_owned(ptr: *mut iree::iree_hal_pool_t) -> Option<Self> {
        NonNull::new(ptr).map(Self)
    }
    #[inline]
    pub fn as_ptr(&self) -> *mut iree::iree_hal_pool_t {
        self.0.as_ptr()
    }
}

impl Drop for HalPool {
    #[inline]
    fn drop(&mut self) {
        // SAFETY: self owns one reference; release it once.
        unsafe { ireei::iree_hal_pool_release(self.0.as_ptr()) };
    }
}
