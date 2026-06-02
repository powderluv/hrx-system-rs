//! Rust port of libhrx/src/libhrx/pool.c — the "exact" IREE pool used by
//! hrx_buffer_allocate. It is a minimal iree_hal_pool_t implementation whose
//! reservations are backed 1:1 by iree_hal_allocator_allocate_buffer calls of
//! exactly the requested size (no suballocation). IREE's queue_alloca drives
//! the vtable below.
#![allow(non_snake_case)]

use core::ffi::c_void;
use core::sync::atomic::AtomicI32;

use iree_sys as iree;
use iree_sys::init as ireei;

// --- IREE pool ABI models (probed against runtime headers) ---

/// `iree_hal_resource_t` (16B): ref_count (atomic i32) @0, vtable ptr @8.
#[repr(C)]
struct IreeHalResource {
    ref_count: AtomicI32,
    _pad: u32,
    vtable: *const c_void,
}

/// `iree_hal_pool_reservation_t` (32B).
#[repr(C)]
struct IreeHalPoolReservation {
    offset: u64,
    length: u64,
    block_handle: u64,
    slab_index: u16,
    reserved: [u16; 3],
}

/// `iree_hal_pool_acquire_info_t` (16B).
#[repr(C)]
struct IreeHalPoolAcquireInfo {
    wait_frontier: *const c_void,
    flags: u32,
    reserved: u32,
}

/// `iree_hal_pool_capabilities_t` (24B).
#[repr(C)]
struct IreeHalPoolCapabilities {
    memory_type: u32,
    supported_usage: u32,
    min_allocation_size: u64,
    max_allocation_size: u64,
}

/// `iree_hal_pool_vtable_t` (64B): 8 function pointers, destroy first (which is
/// also the resource vtable IREE_HAL_VTABLE_DISPATCH reads at offset 0).
#[repr(C)]
struct IreeHalPoolVtable {
    destroy: unsafe extern "C" fn(*mut c_void),
    acquire_reservation: unsafe extern "C" fn(
        *mut c_void,
        u64,
        u64,
        *const c_void,
        u32,
        *mut IreeHalPoolReservation,
        *mut IreeHalPoolAcquireInfo,
        *mut u32,
    ) -> iree::iree_status_t,
    release_reservation: unsafe extern "C" fn(*mut c_void, *const IreeHalPoolReservation, *const c_void),
    materialize_reservation: unsafe extern "C" fn(
        *mut c_void,
        ireei::iree_hal_buffer_params_t,
        *const IreeHalPoolReservation,
        u32,
        *mut *mut ireei::iree_hal_buffer_t,
    ) -> iree::iree_status_t,
    query_capabilities: unsafe extern "C" fn(*const c_void, *mut IreeHalPoolCapabilities),
    query_stats: unsafe extern "C" fn(*const c_void, *mut c_void),
    trim: unsafe extern "C" fn(*mut c_void) -> iree::iree_status_t,
    notification: unsafe extern "C" fn(*mut c_void) -> *mut iree::iree_async_notification_t,
}
// All-fn-pointer struct is Sync; safe to place in a static.
unsafe impl Sync for IreeHalPoolVtable {}

/// `hrx_iree_exact_pool_t` — resource header first (so IREE finds the vtable),
/// then our fields.
#[repr(C)]
struct HrxExactPool {
    resource: IreeHalResource,
    host_allocator: iree::iree_allocator_t,
    allocator: *mut iree::iree_hal_allocator_t,
    params: ireei::iree_hal_buffer_params_t,
    notification: *mut iree::iree_async_notification_t,
}

const IREE_HAL_POOL_ACQUIRE_OK_FRESH: u32 = 1;
const IREE_HAL_POOL_MATERIALIZE_FLAG_TRANSFER_RESERVATION_OWNERSHIP: u32 = 1 << 0;

fn params_match(
    lhs: &ireei::iree_hal_buffer_params_t,
    rhs: &ireei::iree_hal_buffer_params_t,
) -> bool {
    lhs.type_ == rhs.type_
        && lhs.access == rhs.access
        && lhs.usage == rhs.usage
        && lhs.queue_affinity == rhs.queue_affinity
        && lhs.min_alignment == rhs.min_alignment
}

static VTABLE: IreeHalPoolVtable = IreeHalPoolVtable {
    destroy: pool_destroy,
    acquire_reservation: pool_acquire_reservation,
    release_reservation: pool_release_reservation,
    materialize_reservation: pool_materialize_reservation,
    query_capabilities: pool_query_capabilities,
    query_stats: pool_query_stats,
    trim: pool_trim,
    notification: pool_notification,
};

unsafe extern "C" fn pool_destroy(base: *mut c_void) {
    let pool = base as *mut HrxExactPool;
    ireei::iree_async_notification_release((*pool).notification);
    ireei::iree_hal_allocator_release((*pool).allocator);
    iree::iree_allocator_free((*pool).host_allocator, base);
}

unsafe extern "C" fn pool_acquire_reservation(
    base: *mut c_void,
    size: u64,
    _alignment: u64,
    _requester_frontier: *const c_void,
    _flags: u32,
    out_reservation: *mut IreeHalPoolReservation,
    out_info: *mut IreeHalPoolAcquireInfo,
    out_result: *mut u32,
) -> iree::iree_status_t {
    let pool = base as *mut HrxExactPool;
    if size == 0 {
        return iree::iree_status_from_code(iree::IREE_STATUS_INVALID_ARGUMENT);
    }
    core::ptr::write_bytes(out_reservation as *mut u8, 0, core::mem::size_of::<IreeHalPoolReservation>());
    core::ptr::write_bytes(out_info as *mut u8, 0, core::mem::size_of::<IreeHalPoolAcquireInfo>());
    let mut buffer: *mut ireei::iree_hal_buffer_t = core::ptr::null_mut();
    let s = ireei::iree_hal_allocator_allocate_buffer((*pool).allocator, (*pool).params, size, &mut buffer);
    if !iree::status_is_ok(s) {
        return s;
    }
    (*out_reservation).length = ireei::iree_hal_buffer_byte_length(buffer);
    (*out_reservation).block_handle = buffer as u64;
    *out_result = IREE_HAL_POOL_ACQUIRE_OK_FRESH;
    core::ptr::null_mut()
}

unsafe extern "C" fn pool_release_reservation(
    base: *mut c_void,
    reservation: *const IreeHalPoolReservation,
    _death_frontier: *const c_void,
) {
    let pool = base as *mut HrxExactPool;
    let buffer = (*reservation).block_handle as *mut ireei::iree_hal_buffer_t;
    if !buffer.is_null() {
        ireei::iree_hal_buffer_release(buffer);
        ireei::iree_async_notification_signal((*pool).notification, 1);
    }
}

unsafe extern "C" fn pool_materialize_reservation(
    base: *mut c_void,
    params: ireei::iree_hal_buffer_params_t,
    reservation: *const IreeHalPoolReservation,
    flags: u32,
    out_buffer: *mut *mut ireei::iree_hal_buffer_t,
) -> iree::iree_status_t {
    let pool = base as *mut HrxExactPool;
    *out_buffer = core::ptr::null_mut();
    if !params_match(&(*pool).params, &params) {
        return iree::iree_status_from_code(iree::IREE_STATUS_INVALID_ARGUMENT);
    }
    let buffer = (*reservation).block_handle as *mut ireei::iree_hal_buffer_t;
    if buffer.is_null() {
        return iree::iree_status_from_code(iree::IREE_STATUS_INVALID_ARGUMENT);
    }
    if flags & IREE_HAL_POOL_MATERIALIZE_FLAG_TRANSFER_RESERVATION_OWNERSHIP == 0 {
        ireei::iree_hal_buffer_retain(buffer);
    }
    *out_buffer = buffer;
    core::ptr::null_mut()
}

unsafe extern "C" fn pool_query_capabilities(base: *const c_void, out_capabilities: *mut IreeHalPoolCapabilities) {
    let pool = base as *const HrxExactPool;
    (*out_capabilities).memory_type = (*pool).params.type_;
    (*out_capabilities).supported_usage = (*pool).params.usage;
    (*out_capabilities).min_allocation_size = 0;
    (*out_capabilities).max_allocation_size = 0;
}

unsafe extern "C" fn pool_query_stats(_base: *const c_void, out_stats: *mut c_void) {
    // iree_hal_pool_stats_t is 104B; zero it.
    core::ptr::write_bytes(out_stats as *mut u8, 0, 104);
}

unsafe extern "C" fn pool_trim(_base: *mut c_void) -> iree::iree_status_t {
    core::ptr::null_mut()
}

unsafe extern "C" fn pool_notification(base: *mut c_void) -> *mut iree::iree_async_notification_t {
    let pool = base as *mut HrxExactPool;
    (*pool).notification
}

/// `hrx_iree_exact_pool_create` — mirrors pool.c. Returns the pool as an opaque
/// `iree_hal_pool_t*` (really `*mut HrxExactPool`). The caller owns one ref and
/// releases it with iree_hal_pool_release.
pub(crate) unsafe fn hrx_iree_exact_pool_create(
    allocator: *mut iree::iree_hal_allocator_t,
    params: ireei::iree_hal_buffer_params_t,
    out_pool: *mut *mut iree::iree_hal_pool_t,
) -> iree::iree_status_t {
    *out_pool = core::ptr::null_mut();

    let (proactor_pool, host_allocator) = crate::runtime::shared_proactor_and_host_allocator();
    if proactor_pool.is_null() {
        return iree::iree_status_from_code(iree::IREE_STATUS_FAILED_PRECONDITION);
    }

    let mut proactor: *mut iree::iree_async_proactor_t = core::ptr::null_mut();
    let s = ireei::iree_async_proactor_pool_get(proactor_pool, 0, &mut proactor);
    if !iree::status_is_ok(s) {
        return s;
    }

    let mut pool: *mut HrxExactPool = core::ptr::null_mut();
    let s = iree::iree_allocator_malloc(
        host_allocator,
        core::mem::size_of::<HrxExactPool>(),
        &mut pool as *mut *mut HrxExactPool as *mut *mut c_void,
    );
    if !iree::status_is_ok(s) {
        return s;
    }
    core::ptr::write_bytes(pool as *mut u8, 0, core::mem::size_of::<HrxExactPool>());
    // iree_hal_resource_initialize: ref_count=1, vtable=&VTABLE.
    (*pool).resource.ref_count = AtomicI32::new(1);
    (*pool).resource.vtable = &VTABLE as *const IreeHalPoolVtable as *const c_void;
    (*pool).host_allocator = host_allocator;
    (*pool).allocator = allocator;
    (*pool).params = params;
    ireei::iree_hal_allocator_retain(allocator);

    let s = ireei::iree_async_notification_create(proactor, 0, &mut (*pool).notification);
    if !iree::status_is_ok(s) {
        ireei::iree_hal_allocator_release((*pool).allocator);
        iree::iree_allocator_free(host_allocator, pool as *mut c_void);
        return s;
    }

    *out_pool = pool as *mut iree::iree_hal_pool_t;
    core::ptr::null_mut()
}

// Compile-time ABI guards for the IREE pool/resource structs IREE dereferences.
const _: () = {
    assert!(core::mem::size_of::<IreeHalResource>() == 16);
    assert!(core::mem::size_of::<IreeHalPoolReservation>() == 32);
    assert!(core::mem::size_of::<IreeHalPoolAcquireInfo>() == 16);
    assert!(core::mem::size_of::<IreeHalPoolCapabilities>() == 24);
    assert!(core::mem::size_of::<IreeHalPoolVtable>() == 64);
    // The IREE resource header (vtable dispatch) must be at offset 0 of the pool.
    assert!(core::mem::offset_of!(HrxExactPool, resource) == 0);
};
