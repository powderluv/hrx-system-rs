//! Rust port of libhrx/src/libhrx/{allocator,buffer,transfer}.c — the memory
//! path. The allocator-based allocate/import, virtual/physical memory, buffer
//! map/unmap/ptr/size/lifetime, synchronous h2d/d2h transfers, and the
//! stream-ordered hrx_buffer_allocate (queue alloca + hrx exact pool) are ported.
#![allow(non_snake_case)]

#[allow(unused_imports)] use core::ffi::c_void;
use core::sync::atomic::{AtomicI32, Ordering};

use crate::common::*;
use crate::device::{hrx_device_release, hrx_device_retain, HrxAllocatorInline, HrxDevice};
use crate::pool::hrx_iree_exact_pool_create;
use crate::stream::{hrx_stream_flush, HrxStream};
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

/// `hrx_buffer_s` (hrx_internal.h):
/// { ref_count, hal_buffer, hal_pool, device, mem_type, size,
///   mapping (iree_hal_buffer_mapping_t, 48B), is_mapped, mapped_ptr }.
#[repr(C)]
pub struct HrxBufferS {
    pub ref_count: AtomicI32,
    pub hal_buffer: *mut ireei::iree_hal_buffer_t,
    pub hal_pool: *mut c_void, // iree_hal_pool_t* (set on the stream-alloca path)
    pub device: HrxDevice,
    pub mem_type: u32,
    pub size: usize,
    pub mapping: ireei::iree_hal_buffer_mapping_t,
    pub is_mapped: bool,
    pub mapped_ptr: *mut c_void,
}

pub type HrxBuffer = *mut HrxBufferS;
pub type HrxAllocator = *mut HrxAllocatorInline;

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

/// Allocate a hrx_buffer_s on the C heap (libc malloc) so the C reference and
/// Rust free it identically (iree_allocator_free -> libc free), and so the box
/// isn't tracked by Rust's allocator.
unsafe fn alloc_buffer_struct() -> *mut HrxBufferS {
    let p = libc::calloc(1, core::mem::size_of::<HrxBufferS>()) as *mut HrxBufferS;
    p
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
    let buf = alloc_buffer_struct();
    if buf.is_null() {
        ireei::iree_hal_buffer_release(hal_buffer);
        return hrx_make_status(HrxStatusCode::OutOfMemory as i32, c"out of memory".as_ptr());
    }
    (*buf).ref_count = AtomicI32::new(1);
    (*buf).hal_buffer = hal_buffer;
    (*buf).device = (*allocator).device;
    hrx_device_retain((*buf).device);
    (*buf).mem_type = params.type_;
    (*buf).size = size;
    *buffer = buf;
    hrx_ok_status()
}

// --- buffer ops ---

#[no_mangle]
pub unsafe extern "C" fn hrx_buffer_retain(buffer: HrxBuffer) {
    ireei::iree_hal_buffer_retain((*buffer).hal_buffer);
    // hal_pool is non-null only on the stream-alloca path; iree_hal_pool_retain
    // is NULL-safe, matching the C unconditional call.
    ireei::iree_hal_pool_retain((*buffer).hal_pool);
    hrx_device_retain((*buffer).device);
    (*buffer).ref_count.fetch_add(1, Ordering::Relaxed);
}

unsafe fn buffer_unmap_internal(buffer: HrxBuffer) -> iree::iree_status_t {
    let s = ireei::iree_hal_buffer_unmap_range(&mut (*buffer).mapping);
    (*buffer).is_mapped = false;
    (*buffer).mapped_ptr = core::ptr::null_mut();
    s
}

#[no_mangle]
pub unsafe extern "C" fn hrx_buffer_release(buffer: HrxBuffer) {
    let hal_buffer = (*buffer).hal_buffer;
    let hal_pool = (*buffer).hal_pool;
    let device = (*buffer).device;
    if (*buffer).ref_count.fetch_sub(1, Ordering::AcqRel) == 1 {
        if (*buffer).is_mapped {
            let s = buffer_unmap_internal(buffer);
            iree::iree_status_free(s);
        }
        libc::free(buffer as *mut c_void);
    }
    ireei::iree_hal_buffer_release(hal_buffer);
    ireei::iree_hal_pool_release(hal_pool); // NULL-safe; matches C
    hrx_device_release(device);
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
    if (*buffer).is_mapped {
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
    let s = ireei::iree_hal_buffer_map_range(
        (*buffer).hal_buffer,
        ireei::IREE_HAL_MAPPING_MODE_SCOPED,
        access,
        offset as u64,
        size as u64,
        &mut (*buffer).mapping,
    );
    if !iree::status_is_ok(s) {
        return hrx_status_from_iree(s);
    }
    (*buffer).is_mapped = true;
    (*buffer).mapped_ptr = (*buffer).mapping.contents.data as *mut c_void;
    *mapped_ptr = (*buffer).mapping.contents.data as *mut c_void;
    hrx_ok_status()
}

#[no_mangle]
pub unsafe extern "C" fn hrx_buffer_unmap(buffer: HrxBuffer) -> HrxStatus {
    if buffer.is_null() {
        return hrx_make_status(HrxStatusCode::InvalidArgument as i32, c"buffer is NULL".as_ptr());
    }
    if !(*buffer).is_mapped {
        return hrx_ok_status(); // not mapped, no-op
    }
    hrx_status_from_iree(buffer_unmap_internal(buffer))
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
    if !(*buffer).mapped_ptr.is_null() {
        *device_ptr = (*buffer).mapped_ptr;
        return hrx_ok_status();
    }
    let s = ireei::iree_hal_buffer_map_range(
        (*buffer).hal_buffer,
        ireei::IREE_HAL_MAPPING_MODE_SCOPED,
        ireei::IREE_HAL_MEMORY_ACCESS_ALL,
        0,
        (*buffer).size as u64,
        &mut (*buffer).mapping,
    );
    if iree::status_is_ok(s) {
        (*buffer).is_mapped = true;
        (*buffer).mapped_ptr = (*buffer).mapping.contents.data as *mut c_void;
        *device_ptr = (*buffer).mapped_ptr;
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
    *size = (*buffer).size;
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
    if dst_offset + size > (*dst).size {
        return hrx_make_status(
            HrxStatusCode::OutOfRange as i32,
            c"transfer exceeds buffer size".as_ptr(),
        );
    }
    hrx_status_from_iree(ireei::iree_hal_device_transfer_h2d(
        (*device).hal_device.as_ptr(),
        host_src,
        (*dst).hal_buffer,
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
    if src_offset + size > (*src).size {
        return hrx_make_status(
            HrxStatusCode::OutOfRange as i32,
            c"transfer exceeds buffer size".as_ptr(),
        );
    }
    hrx_status_from_iree(ireei::iree_hal_device_transfer_d2h(
        (*device).hal_device.as_ptr(),
        (*src).hal_buffer,
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
    let buf = alloc_buffer_struct();
    if buf.is_null() {
        ireei::iree_hal_buffer_release(hal_buffer);
        return hrx_make_status(HrxStatusCode::OutOfMemory as i32, c"out of memory".as_ptr());
    }
    (*buf).ref_count = AtomicI32::new(1);
    (*buf).hal_buffer = hal_buffer;
    (*buf).device = (*allocator).device;
    hrx_device_retain((*buf).device);
    (*buf).mem_type = params.type_;
    (*buf).size = size;
    *buffer = buf;
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
    let buf = alloc_buffer_struct();
    if buf.is_null() {
        fem::iree_hal_allocator_virtual_memory_release((*allocator).hal_allocator.as_ptr(), hal_buffer);
        return hrx_make_status(HrxStatusCode::OutOfMemory as i32, c"out of memory".as_ptr());
    }
    (*buf).ref_count = AtomicI32::new(1);
    (*buf).hal_buffer = hal_buffer;
    (*buf).device = (*allocator).device;
    hrx_device_retain((*buf).device);
    (*buf).mem_type = 0x30; // HRX_MEMORY_TYPE_DEVICE_LOCAL
    (*buf).size = size;
    *virtual_buffer = buf;
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
    let s = fem::iree_hal_allocator_virtual_memory_release((*allocator).hal_allocator.as_ptr(), (*virtual_buffer).hal_buffer);
    // hal_buffer ownership transferred; free the hrx wrapper.
    (*virtual_buffer).hal_buffer = core::ptr::null_mut();
    hrx_device_release((*virtual_buffer).device);
    libc::free(virtual_buffer as *mut c_void);
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
        (*virtual_buffer).hal_buffer,
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
        (*virtual_buffer).hal_buffer,
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
        (*virtual_buffer).hal_buffer,
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

    let buf = alloc_buffer_struct();
    if buf.is_null() {
        return hrx_make_status(HrxStatusCode::OutOfMemory as i32, c"out of memory".as_ptr());
    }

    let allocator = (*(*stream).device).allocator.hal_allocator.as_ptr();
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
        libc::free(buf as *mut c_void);
        return hrx_make_status(HrxStatusCode::InvalidArgument as i32, c"buffer params are not allocatable on this device".as_ptr());
    }

    let flush_status = hrx_stream_flush(stream);
    if !hrx_status_is_ok(flush_status) {
        libc::free(buf as *mut c_void);
        return flush_status;
    }

    let mut wait_value = (*stream).timepoint;
    let mut signal_value = (*stream).timepoint + 1;
    let mut sem = crate::semaphore::semaphore_hal_ptr((*stream).semaphore);
    let wait_list = ireei::iree_hal_semaphore_list_t {
        count: if (*stream).timepoint > 0 { 1 } else { 0 },
        semaphores: &mut sem,
        payload_values: &mut wait_value,
    };
    let signal_list = ireei::iree_hal_semaphore_list_t {
        count: 1,
        semaphores: &mut sem,
        payload_values: &mut signal_value,
    };

    let mut status = hrx_iree_exact_pool_create(allocator, params, &mut (*buf).hal_pool);
    if iree::status_is_ok(status) {
        status = ireei::iree_hal_device_queue_alloca(
            (*(*stream).device).hal_device.as_ptr(),
            ireei::IREE_HAL_QUEUE_AFFINITY_ANY,
            wait_list,
            signal_list,
            (*buf).hal_pool,
            params,
            size as u64,
            0, // IREE_HAL_ALLOCA_FLAG_NONE
            &mut (*buf).hal_buffer,
        );
    }
    if iree::status_is_ok(status) {
        // The AMDGPU transient allocator resolves committed backing while
        // recording later command buffer ops; make the queued alloca visible now.
        status = ireei::iree_hal_semaphore_wait(sem, signal_value, ireei::iree_timeout_t::infinite(), 0);
    }
    if !iree::status_is_ok(status) {
        ireei::iree_hal_buffer_release((*buf).hal_buffer);
        ireei::iree_hal_pool_release((*buf).hal_pool);
        libc::free(buf as *mut c_void);
        return hrx_status_from_iree(status);
    }

    (*buf).ref_count = AtomicI32::new(1);
    (*buf).device = (*stream).device;
    hrx_device_retain((*buf).device);
    (*buf).mem_type = mem_type;
    (*buf).size = size;
    (*buf).mapped_ptr = core::ptr::null_mut();
    (*stream).timepoint = signal_value;

    *buffer = buf;
    hrx_ok_status()
}
