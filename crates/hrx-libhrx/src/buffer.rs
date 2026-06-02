//! Rust port of libhrx/src/libhrx/{allocator,buffer,transfer}.c — the memory
//! path. The allocator-based allocate/import, virtual/physical memory, buffer
//! map/unmap/ptr/size/lifetime, synchronous h2d/d2h transfers, and the
//! stream-ordered hrx_buffer_allocate (queue alloca + hrx exact pool) are ported.
//!
//! Phase-2 owned model: the opaque `hrx_buffer_t` is the `Arc` data pointer of an
//! `HrxBufferS`. retain/release are `Arc` refcount ops, and on the last release
//! the fields drop in declaration order — `map` (unmap if still mapped) →
//! `hal_buffer` → `hal_pool` → `device` — which reproduces the C release sequence.
//! The mutable map state lives in a `RefCell` (single-threaded, non-atomic, like
//! the C object's plain struct fields). The destructive virtual-memory release
//! takes the object apart by field (`into_inner_handle`) so the HAL buffer can be
//! handed to `iree_hal_allocator_virtual_memory_release` rather than the
//! `iree_hal_buffer_release` that `HalBuffer`'s `Drop` would otherwise call.
#![allow(non_snake_case)]

use core::cell::RefCell;
#[allow(unused_imports)] use core::ffi::c_void;

use crate::common::*;
use crate::device::{hrx_device_release, hrx_device_retain, DeviceRef, HrxAllocatorInline, HrxDevice};
use crate::handle::{handle_ref, handle_release, handle_retain, into_handle, into_inner_handle};
use crate::pool::hrx_iree_exact_pool_create;
use crate::stream::{hrx_stream_flush, stream_device, stream_semaphore, stream_set_timepoint, stream_timepoint, HrxStream};
use iree_hal::{HalBuffer, HalPool};
use iree_sys as iree;
use iree_sys::fem;
use iree_sys::init as ireei;

// Memory access bits (hrx flags == IREE flags, asserted in C).
const HRX_MAP_READ: u32 = ireei::IREE_HAL_MEMORY_ACCESS_READ as u32;
const HRX_MAP_WRITE: u32 = ireei::IREE_HAL_MEMORY_ACCESS_WRITE as u32;
const HRX_MAP_DISCARD: u32 = ireei::IREE_HAL_MEMORY_ACCESS_DISCARD as u32;

/// `hrx_buffer_params_t` (public header, 24 B probed): type u32 @0, access u16
/// @4, usage u32 @8, queue_affinity u64 @16 (hrx_memory_access_t is uint16_t).
#[repr(C)]
#[derive(Clone, Copy)]
pub struct HrxBufferParams {
    pub type_: u32,
    pub access: u16,
    pub _pad0: u16,
    pub usage: u32,
    pub _pad1: u32,
    pub queue_affinity: u64,
}

/// A live scoped mapping. Holds the IREE `mapping` storage (self-contained — IREE
/// keeps its bookkeeping inside the struct, so it may be moved into the heap) and
/// the mapped data pointer. `Drop` unmaps it: this fires only when the buffer is
/// dropped while still mapped, matching C's release-path `iree_status_ignore`
/// unmap. The explicit `hrx_buffer_unmap` path takes the value out and forgets it
/// after unmapping (so the status can be returned and `Drop` does not unmap twice).
struct MappedRange {
    mapping: ireei::iree_hal_buffer_mapping_t,
    ptr: *mut c_void,
}

impl Drop for MappedRange {
    fn drop(&mut self) {
        // SAFETY: `mapping` was filled by a successful scoped map of this buffer;
        // the status is freed (ignored), matching C's release-path unmap.
        unsafe {
            let s = ireei::iree_hal_buffer_unmap_range(&mut self.mapping);
            iree::iree_status_free(s);
        }
    }
}

/// `hrx_buffer_s` — the internal object behind the opaque `hrx_buffer_t`.
/// Field/declaration order is load-bearing for drop: `map` (unmap) → `hal_buffer`
/// → `hal_pool` → `device`, matching the C release order. `HrxBufferS` has no
/// explicit `Drop` so the virtual-memory release path can move fields out of it.
pub struct HrxBufferS {
    map: RefCell<Option<MappedRange>>,
    hal_buffer: HalBuffer,
    /// Non-`None` only on the stream-alloca path (a transient per-allocation pool).
    hal_pool: Option<HalPool>,
    device: DeviceRef,
    /// Kept only to mirror the C `hrx_buffer_s` field set; no ported API reads it.
    #[allow(dead_code)]
    mem_type: u32,
    size: usize,
}

pub type HrxBuffer = *mut HrxBufferS;
pub type HrxAllocator = *mut HrxAllocatorInline;

/// Borrow the raw IREE buffer pointer behind a handle (for queue/stream/value-list
/// submission, which build IREE buffer refs).
///
/// # Safety
/// `buffer` must be a live `hrx_buffer_t`.
pub(crate) unsafe fn buffer_hal(buffer: HrxBuffer) -> *mut ireei::iree_hal_buffer_t {
    handle_ref(buffer).hal_buffer.as_ptr()
}

// --- allocator ops ---

#[no_mangle]
pub unsafe extern "C" fn hrx_device_allocator(device: HrxDevice) -> HrxAllocator {
    &mut (*device).allocator as HrxAllocator
}

#[no_mangle]
pub unsafe extern "C" fn hrx_allocator_retain(allocator: HrxAllocator) {
    ireei::iree_hal_allocator_retain((*allocator).hal_allocator.as_ptr());
    hrx_device_retain((*allocator).device);
}

#[no_mangle]
pub unsafe extern "C" fn hrx_allocator_release(allocator: HrxAllocator) {
    ireei::iree_hal_allocator_release((*allocator).hal_allocator.as_ptr());
    hrx_device_release((*allocator).device);
}

#[no_mangle]
pub unsafe extern "C" fn hrx_allocator_allocate_buffer(
    allocator: HrxAllocator,
    params: HrxBufferParams,
    size: usize,
    buffer: *mut HrxBuffer,
) -> HrxStatus {
    if allocator.is_null() || buffer.is_null() {
        return hrx_make_status(
            HrxStatusCode::InvalidArgument as i32,
            c"allocator or buffer is NULL".as_ptr(),
        );
    }
    let hal_params = ireei::iree_hal_buffer_params_t {
        usage: params.usage,
        access: params.access,
        _pad0: 0,
        type_: params.type_,
        _pad1: 0,
        queue_affinity: params.queue_affinity,
        min_alignment: 0,
    };
    let mut hal_buffer: *mut ireei::iree_hal_buffer_t = core::ptr::null_mut();
    let s = ireei::iree_hal_allocator_allocate_buffer(
        (*allocator).hal_allocator.as_ptr(),
        hal_params,
        size as u64,
        &mut hal_buffer,
    );
    if !iree::status_is_ok(s) {
        return hrx_status_from_iree(s);
    }
    let hal_buffer = HalBuffer::from_owned(hal_buffer).expect("OK alloc with null buffer");
    let device = DeviceRef::retain((*allocator).device);
    *buffer = into_handle(HrxBufferS {
        map: RefCell::new(None),
        hal_buffer,
        hal_pool: None,
        device,
        mem_type: params.type_,
        size,
    });
    hrx_ok_status()
}

// --- buffer ops ---

#[no_mangle]
pub unsafe extern "C" fn hrx_buffer_retain(buffer: HrxBuffer) {
    handle_retain(buffer);
}

#[no_mangle]
pub unsafe extern "C" fn hrx_buffer_release(buffer: HrxBuffer) {
    // The HAL teardown (unmap-if-mapped, release hal_buffer/pool/device) moved into
    // the field drops, which run on the last reference. The C code released the HAL
    // objects on every call to balance per-retain HAL retains; the owned model
    // holds one reference each for the buffer's lifetime and releases them once on
    // drop — observably equivalent.
    handle_release(buffer);
}

#[no_mangle]
pub unsafe extern "C" fn hrx_buffer_map(
    buffer: HrxBuffer,
    flags: u32,
    offset: usize,
    size: usize,
    mapped_ptr: *mut *mut c_void,
) -> HrxStatus {
    if buffer.is_null() || mapped_ptr.is_null() {
        return hrx_make_status(
            HrxStatusCode::InvalidArgument as i32,
            c"buffer or mapped_ptr is NULL".as_ptr(),
        );
    }
    let buf = handle_ref(buffer);
    let mut map = buf.map.borrow_mut();
    if map.is_some() {
        return hrx_make_status(
            HrxStatusCode::FailedPrecondition as i32,
            c"buffer is already mapped".as_ptr(),
        );
    }
    let mut access: u16 = 0;
    if flags & HRX_MAP_READ != 0 {
        access |= ireei::IREE_HAL_MEMORY_ACCESS_READ;
    }
    if flags & HRX_MAP_WRITE != 0 {
        access |= ireei::IREE_HAL_MEMORY_ACCESS_WRITE;
    }
    if flags & HRX_MAP_DISCARD != 0 {
        access |= ireei::IREE_HAL_MEMORY_ACCESS_DISCARD_WRITE;
    }
    // Map into a stack temporary, then move it into the heap. The IREE mapping is
    // self-contained (no captured address), so the move is sound.
    let mut mapping = ireei::iree_hal_buffer_mapping_t::zeroed();
    let s = ireei::iree_hal_buffer_map_range(
        buf.hal_buffer.as_ptr(),
        ireei::IREE_HAL_MAPPING_MODE_SCOPED,
        access,
        offset as u64,
        size as u64,
        &mut mapping,
    );
    if !iree::status_is_ok(s) {
        return hrx_status_from_iree(s);
    }
    let ptr = mapping.contents.data as *mut c_void;
    *map = Some(MappedRange { mapping, ptr });
    *mapped_ptr = ptr;
    hrx_ok_status()
}

#[no_mangle]
pub unsafe extern "C" fn hrx_buffer_unmap(buffer: HrxBuffer) -> HrxStatus {
    if buffer.is_null() {
        return hrx_make_status(HrxStatusCode::InvalidArgument as i32, c"buffer is NULL".as_ptr());
    }
    let prev = handle_ref(buffer).map.borrow_mut().take();
    match prev {
        None => hrx_ok_status(), // not mapped, no-op
        Some(mut m) => {
            let s = ireei::iree_hal_buffer_unmap_range(&mut m.mapping);
            // Already unmapped here; suppress the MappedRange Drop's second unmap.
            core::mem::forget(m);
            hrx_status_from_iree(s)
        }
    }
}

#[no_mangle]
pub unsafe extern "C" fn hrx_buffer_get_device_ptr(
    buffer: HrxBuffer,
    device_ptr: *mut *mut c_void,
) -> HrxStatus {
    if buffer.is_null() || device_ptr.is_null() {
        return hrx_make_status(
            HrxStatusCode::InvalidArgument as i32,
            c"buffer or device_ptr is NULL".as_ptr(),
        );
    }
    let buf = handle_ref(buffer);
    let mut map = buf.map.borrow_mut();
    if let Some(m) = map.as_ref() {
        *device_ptr = m.ptr;
        return hrx_ok_status();
    }
    let mut mapping = ireei::iree_hal_buffer_mapping_t::zeroed();
    let s = ireei::iree_hal_buffer_map_range(
        buf.hal_buffer.as_ptr(),
        ireei::IREE_HAL_MAPPING_MODE_SCOPED,
        ireei::IREE_HAL_MEMORY_ACCESS_ALL,
        0,
        buf.size as u64,
        &mut mapping,
    );
    if iree::status_is_ok(s) {
        let ptr = mapping.contents.data as *mut c_void;
        *map = Some(MappedRange { mapping, ptr });
        *device_ptr = ptr;
        return hrx_ok_status();
    }
    iree::iree_status_free(s);
    *device_ptr = core::ptr::null_mut();
    hrx_make_status(
        HrxStatusCode::Unavailable as i32,
        c"cannot get device pointer for this buffer type".as_ptr(),
    )
}

#[no_mangle]
pub unsafe extern "C" fn hrx_buffer_get_size(buffer: HrxBuffer, size: *mut usize) -> HrxStatus {
    if buffer.is_null() || size.is_null() {
        return hrx_make_status(
            HrxStatusCode::InvalidArgument as i32,
            c"buffer or size is NULL".as_ptr(),
        );
    }
    *size = handle_ref(buffer).size;
    hrx_ok_status()
}

// --- synchronous transfers (transfer.c) ---

#[no_mangle]
pub unsafe extern "C" fn hrx_synchronous_h2d(
    device: HrxDevice,
    host_src: *const c_void,
    dst: HrxBuffer,
    dst_offset: usize,
    size: usize,
) -> HrxStatus {
    if device.is_null() || host_src.is_null() || dst.is_null() {
        return hrx_make_status(HrxStatusCode::InvalidArgument as i32, c"NULL argument".as_ptr());
    }
    let dst_buf = handle_ref(dst);
    if dst_offset + size > dst_buf.size {
        return hrx_make_status(
            HrxStatusCode::OutOfRange as i32,
            c"transfer exceeds buffer size".as_ptr(),
        );
    }
    hrx_status_from_iree(ireei::iree_hal_device_transfer_h2d(
        (*device).hal_device.as_ptr(),
        host_src,
        dst_buf.hal_buffer.as_ptr(),
        dst_offset as u64,
        size as u64,
        ireei::IREE_HAL_TRANSFER_BUFFER_FLAG_DEFAULT,
        ireei::iree_timeout_t::infinite(),
    ))
}

#[no_mangle]
pub unsafe extern "C" fn hrx_synchronous_d2h(
    device: HrxDevice,
    src: HrxBuffer,
    src_offset: usize,
    host_dst: *mut c_void,
    size: usize,
) -> HrxStatus {
    if device.is_null() || src.is_null() || host_dst.is_null() {
        return hrx_make_status(HrxStatusCode::InvalidArgument as i32, c"NULL argument".as_ptr());
    }
    let src_buf = handle_ref(src);
    if src_offset + size > src_buf.size {
        return hrx_make_status(
            HrxStatusCode::OutOfRange as i32,
            c"transfer exceeds buffer size".as_ptr(),
        );
    }
    hrx_status_from_iree(ireei::iree_hal_device_transfer_d2h(
        (*device).hal_device.as_ptr(),
        src_buf.hal_buffer.as_ptr(),
        src_offset as u64,
        host_dst,
        size as u64,
        ireei::IREE_HAL_TRANSFER_BUFFER_FLAG_DEFAULT,
        ireei::iree_timeout_t::infinite(),
    ))
}

// --- import + virtual/physical memory (allocator.c remainder) ---

/// hrx_physical_memory_t is an opaque pointer to the IREE physical memory.
pub type HrxPhysicalMemory = *mut ireei::iree_hal_physical_memory_t;

#[no_mangle]
pub unsafe extern "C" fn hrx_allocator_import_buffer(
    allocator: HrxAllocator,
    params: HrxBufferParams,
    host_ptr: *mut c_void,
    size: usize,
    buffer: *mut HrxBuffer,
) -> HrxStatus {
    if allocator.is_null() || host_ptr.is_null() || buffer.is_null() {
        return hrx_make_status(HrxStatusCode::InvalidArgument as i32, c"allocator, host_ptr, or buffer is NULL".as_ptr());
    }
    let hal_params = ireei::iree_hal_buffer_params_t {
        usage: params.usage,
        access: params.access,
        _pad0: 0,
        type_: params.type_,
        _pad1: 0,
        queue_affinity: params.queue_affinity,
        min_alignment: 0,
    };
    let mut ext = fem::iree_hal_external_buffer_t {
        type_: fem::IREE_HAL_EXTERNAL_BUFFER_TYPE_HOST_ALLOCATION,
        flags: 0,
        size: size as u64,
        handle_ptr: host_ptr,
    };
    let mut hal_buffer: *mut ireei::iree_hal_buffer_t = core::ptr::null_mut();
    let s = fem::iree_hal_allocator_import_buffer(
        (*allocator).hal_allocator.as_ptr(),
        hal_params,
        &mut ext,
        fem::iree_hal_buffer_release_callback_t::null(),
        &mut hal_buffer,
    );
    if !iree::status_is_ok(s) {
        return hrx_status_from_iree(s);
    }
    let hal_buffer = HalBuffer::from_owned(hal_buffer).expect("OK import with null buffer");
    let device = DeviceRef::retain((*allocator).device);
    *buffer = into_handle(HrxBufferS {
        map: RefCell::new(None),
        hal_buffer,
        hal_pool: None,
        device,
        mem_type: params.type_,
        size,
    });
    hrx_ok_status()
}

#[no_mangle]
pub unsafe extern "C" fn hrx_allocator_query_virtual_memory(
    allocator: HrxAllocator,
    mem_type: u32,
    supported: *mut bool,
    min_page_size: *mut usize,
    recommended_page_size: *mut usize,
) -> HrxStatus {
    if allocator.is_null() || supported.is_null() {
        return hrx_make_status(HrxStatusCode::InvalidArgument as i32, c"allocator or supported is NULL".as_ptr());
    }
    *supported = fem::iree_hal_allocator_supports_virtual_memory((*allocator).hal_allocator.as_ptr());
    if !*supported {
        if !min_page_size.is_null() { *min_page_size = 0; }
        if !recommended_page_size.is_null() { *recommended_page_size = 0; }
        return hrx_ok_status();
    }
    // IREE_HAL_BUFFER_USAGE_DEFAULT=0xC03, ACCESS_ALL=7.
    let hal_params = ireei::iree_hal_buffer_params_t {
        usage: 0x0000_0C03,
        access: ireei::IREE_HAL_MEMORY_ACCESS_ALL,
        _pad0: 0,
        type_: mem_type,
        _pad1: 0,
        queue_affinity: 0,
        min_alignment: 0,
    };
    let mut min: u64 = 0;
    let mut rec: u64 = 0;
    let s = fem::iree_hal_allocator_virtual_memory_query_granularity((*allocator).hal_allocator.as_ptr(), hal_params, &mut min, &mut rec);
    if !iree::status_is_ok(s) {
        *supported = false;
        if !min_page_size.is_null() { *min_page_size = 0; }
        if !recommended_page_size.is_null() { *recommended_page_size = 0; }
        iree::iree_status_free(s);
        return hrx_ok_status();
    }
    if !min_page_size.is_null() { *min_page_size = min as usize; }
    if !recommended_page_size.is_null() { *recommended_page_size = rec as usize; }
    hrx_ok_status()
}

#[no_mangle]
pub unsafe extern "C" fn hrx_allocator_virtual_memory_reserve(
    allocator: HrxAllocator,
    affinity: u64,
    size: usize,
    virtual_buffer: *mut HrxBuffer,
) -> HrxStatus {
    if allocator.is_null() || virtual_buffer.is_null() {
        return hrx_make_status(HrxStatusCode::InvalidArgument as i32, c"NULL argument".as_ptr());
    }
    let mut hal_buffer: *mut ireei::iree_hal_buffer_t = core::ptr::null_mut();
    let s = fem::iree_hal_allocator_virtual_memory_reserve((*allocator).hal_allocator.as_ptr(), affinity, size as u64, &mut hal_buffer);
    if !iree::status_is_ok(s) {
        return hrx_status_from_iree(s);
    }
    let hal_buffer = HalBuffer::from_owned(hal_buffer).expect("OK reserve with null buffer");
    let device = DeviceRef::retain((*allocator).device);
    *virtual_buffer = into_handle(HrxBufferS {
        map: RefCell::new(None),
        hal_buffer,
        hal_pool: None,
        device,
        mem_type: 0x30, // HRX_MEMORY_TYPE_DEVICE_LOCAL
        size,
    });
    hrx_ok_status()
}

#[no_mangle]
pub unsafe extern "C" fn hrx_allocator_virtual_memory_release(
    allocator: HrxAllocator,
    virtual_buffer: HrxBuffer,
) -> HrxStatus {
    if allocator.is_null() || virtual_buffer.is_null() {
        return hrx_make_status(HrxStatusCode::InvalidArgument as i32, c"NULL argument".as_ptr());
    }
    // Destructive release: take the object out of its Arc (single owner) and hand
    // the HAL buffer to virtual_memory_release instead of iree_hal_buffer_release.
    let inner = into_inner_handle(virtual_buffer)
        .expect("virtual buffer released with outstanding references");
    let HrxBufferS { map, hal_buffer, hal_pool, device, mem_type: _, size: _ } = inner;
    // `into_raw` forgets the wrapper so its Drop does not also release the buffer.
    let hal_ptr = hal_buffer.into_raw();
    drop(map); // virtual buffers are never mapped; None → no-op
    drop(hal_pool); // None → no-op
    let s = fem::iree_hal_allocator_virtual_memory_release((*allocator).hal_allocator.as_ptr(), hal_ptr);
    drop(device); // hrx_device_release, after virtual_memory_release — matches C order
    hrx_status_from_iree(s)
}

#[no_mangle]
pub unsafe extern "C" fn hrx_allocator_physical_memory_allocate(
    allocator: HrxAllocator,
    mem_type: u32,
    size: usize,
    physical: *mut HrxPhysicalMemory,
) -> HrxStatus {
    if allocator.is_null() || physical.is_null() {
        return hrx_make_status(HrxStatusCode::InvalidArgument as i32, c"NULL argument".as_ptr());
    }
    let hal_params = ireei::iree_hal_buffer_params_t {
        usage: 0x0000_0C03,
        access: ireei::IREE_HAL_MEMORY_ACCESS_ALL,
        _pad0: 0,
        type_: mem_type,
        _pad1: 0,
        queue_affinity: 0,
        min_alignment: 0,
    };
    hrx_status_from_iree(fem::iree_hal_allocator_physical_memory_allocate(
        (*allocator).hal_allocator.as_ptr(),
        hal_params,
        size as u64,
        iree::allocator_system(),
        physical,
    ))
}

#[no_mangle]
pub unsafe extern "C" fn hrx_allocator_physical_memory_free(
    allocator: HrxAllocator,
    physical: HrxPhysicalMemory,
) -> HrxStatus {
    if allocator.is_null() || physical.is_null() {
        return hrx_make_status(HrxStatusCode::InvalidArgument as i32, c"NULL argument".as_ptr());
    }
    hrx_status_from_iree(fem::iree_hal_allocator_physical_memory_free((*allocator).hal_allocator.as_ptr(), physical))
}

#[no_mangle]
pub unsafe extern "C" fn hrx_allocator_virtual_memory_map(
    allocator: HrxAllocator,
    virtual_buffer: HrxBuffer,
    virtual_offset: usize,
    physical: HrxPhysicalMemory,
    physical_offset: usize,
    size: usize,
) -> HrxStatus {
    if allocator.is_null() || virtual_buffer.is_null() || physical.is_null() {
        return hrx_make_status(HrxStatusCode::InvalidArgument as i32, c"NULL argument".as_ptr());
    }
    hrx_status_from_iree(fem::iree_hal_allocator_virtual_memory_map(
        (*allocator).hal_allocator.as_ptr(),
        buffer_hal(virtual_buffer),
        virtual_offset as u64,
        physical,
        physical_offset as u64,
        size as u64,
    ))
}

#[no_mangle]
pub unsafe extern "C" fn hrx_allocator_virtual_memory_unmap(
    allocator: HrxAllocator,
    virtual_buffer: HrxBuffer,
    virtual_offset: usize,
    size: usize,
) -> HrxStatus {
    if allocator.is_null() || virtual_buffer.is_null() {
        return hrx_make_status(HrxStatusCode::InvalidArgument as i32, c"NULL argument".as_ptr());
    }
    hrx_status_from_iree(fem::iree_hal_allocator_virtual_memory_unmap(
        (*allocator).hal_allocator.as_ptr(),
        buffer_hal(virtual_buffer),
        virtual_offset as u64,
        size as u64,
    ))
}

#[no_mangle]
pub unsafe extern "C" fn hrx_allocator_virtual_memory_protect(
    allocator: HrxAllocator,
    virtual_buffer: HrxBuffer,
    virtual_offset: usize,
    size: usize,
    protection: u32,
) -> HrxStatus {
    if allocator.is_null() || virtual_buffer.is_null() {
        return hrx_make_status(HrxStatusCode::InvalidArgument as i32, c"NULL argument".as_ptr());
    }
    hrx_status_from_iree(fem::iree_hal_allocator_virtual_memory_protect(
        (*allocator).hal_allocator.as_ptr(),
        buffer_hal(virtual_buffer),
        virtual_offset as u64,
        size as u64,
        ireei::IREE_HAL_QUEUE_AFFINITY_ANY,
        protection,
    ))
}

// --- stream-ordered allocation (buffer.c hrx_buffer_allocate) ---

#[no_mangle]
pub unsafe extern "C" fn hrx_buffer_allocate(
    stream: HrxStream,
    size: usize,
    mem_type: u32,
    usage: u32,
    buffer: *mut HrxBuffer,
) -> HrxStatus {
    if stream.is_null() || buffer.is_null() {
        return hrx_make_status(HrxStatusCode::InvalidArgument as i32, c"stream or buffer is NULL".as_ptr());
    }
    if size == 0 {
        return hrx_make_status(HrxStatusCode::InvalidArgument as i32, c"allocation size must be > 0".as_ptr());
    }

    let allocator = (*stream_device(stream)).allocator.hal_allocator.as_ptr();
    let mut params = ireei::iree_hal_buffer_params_t {
        usage,
        access: ireei::IREE_HAL_MEMORY_ACCESS_ALL,
        _pad0: 0,
        type_: mem_type,
        _pad1: 0,
        queue_affinity: 0,
        min_alignment: 0,
    };
    // query_buffer_compatibility resolves/normalizes params in place.
    let compatibility = ireei::iree_hal_allocator_query_buffer_compatibility(
        allocator,
        params,
        size as u64,
        &mut params,
        core::ptr::null_mut(),
    );
    if compatibility & ireei::IREE_HAL_BUFFER_COMPATIBILITY_ALLOCATABLE == 0 {
        return hrx_make_status(HrxStatusCode::InvalidArgument as i32, c"buffer params are not allocatable on this device".as_ptr());
    }

    let flush_status = hrx_stream_flush(stream);
    if !hrx_status_is_ok(flush_status) {
        return flush_status;
    }

    let mut wait_value = stream_timepoint(stream);
    let mut signal_value = stream_timepoint(stream) + 1;
    let mut sem = crate::semaphore::semaphore_hal_ptr(stream_semaphore(stream));
    let wait_list = ireei::iree_hal_semaphore_list_t {
        count: if stream_timepoint(stream) > 0 { 1 } else { 0 },
        semaphores: &mut sem,
        payload_values: &mut wait_value,
    };
    let signal_list = ireei::iree_hal_semaphore_list_t {
        count: 1,
        semaphores: &mut sem,
        payload_values: &mut signal_value,
    };

    let mut raw_pool: *mut iree::iree_hal_pool_t = core::ptr::null_mut();
    let mut raw_buffer: *mut ireei::iree_hal_buffer_t = core::ptr::null_mut();
    let mut status = hrx_iree_exact_pool_create(allocator, params, &mut raw_pool);
    if iree::status_is_ok(status) {
        status = ireei::iree_hal_device_queue_alloca(
            (*stream_device(stream)).hal_device.as_ptr(),
            ireei::IREE_HAL_QUEUE_AFFINITY_ANY,
            wait_list,
            signal_list,
            raw_pool,
            params,
            size as u64,
            0, // IREE_HAL_ALLOCA_FLAG_NONE
            &mut raw_buffer,
        );
    }
    if iree::status_is_ok(status) {
        // The AMDGPU transient allocator resolves committed backing while
        // recording later command buffer ops; make the queued alloca visible now.
        status = ireei::iree_hal_semaphore_wait(sem, signal_value, ireei::iree_timeout_t::infinite(), 0);
    }
    if !iree::status_is_ok(status) {
        ireei::iree_hal_buffer_release(raw_buffer); // NULL-safe
        ireei::iree_hal_pool_release(raw_pool); // NULL-safe
        return hrx_status_from_iree(status);
    }

    let hal_buffer = HalBuffer::from_owned(raw_buffer).expect("OK alloca with null buffer");
    let hal_pool = HalPool::from_owned(raw_pool);
    let device = DeviceRef::retain(stream_device(stream));
    stream_set_timepoint(stream, signal_value);
    *buffer = into_handle(HrxBufferS {
        map: RefCell::new(None),
        hal_buffer,
        hal_pool,
        device,
        mem_type,
        size,
    });
    hrx_ok_status()
}
