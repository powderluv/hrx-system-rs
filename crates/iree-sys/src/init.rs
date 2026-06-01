//! FFI for the IREE runtime init/HAL-device-setup path that libhrx's
//! runtime.c / device.c use.
//!
//! Several IREE structs are passed by value or filled by a C initializer. Their
//! exact sizes/alignments were measured against the real headers (abi_probe):
//!   iree_task_topology_t              = 22544 B, align 8
//!   iree_task_executor_options_t      =    40 B, align 8 (worker_stack_size @24)
//!   iree_async_proactor_pool_options_t=    80 B, align 8
//!   iree_async_frontier_tracker_options_t = 8 B, align 4
//!   iree_hal_task_device_params_t     =    24 B, align 8
//!   iree_hal_device_create_params_t   =    16 B, align 8 (proactor_pool @8)
//!   iree_hal_semaphore_list_t         =    24 B, align 8 (zeroed = empty)
//!   iree_string_view_t / iree_timeout_t = 16 B, align 8
//! We model each as a correctly-sized #[repr(C, align(8))] blob, fill it via the
//! C initializer (or zero), and poke only the fields libhrx touches by offset.

use super::*;
use core::ffi::c_void;

pub const IREE_TIME_INFINITE_FUTURE: i64 = i64::MAX;
pub const IREE_TIMEOUT_ABSOLUTE: i32 = 0;

// HAL buffer constants (iree/hal/buffer.h).
pub const IREE_HAL_MAPPING_MODE_SCOPED: u32 = 1;
pub const IREE_HAL_TRANSFER_BUFFER_FLAG_DEFAULT: u32 = 0;
pub const IREE_HAL_MEMORY_ACCESS_READ: u16 = 1 << 0;
pub const IREE_HAL_MEMORY_ACCESS_WRITE: u16 = 1 << 1;
pub const IREE_HAL_MEMORY_ACCESS_DISCARD: u16 = 1 << 2;
pub const IREE_HAL_MEMORY_ACCESS_DISCARD_WRITE: u16 =
    IREE_HAL_MEMORY_ACCESS_WRITE | IREE_HAL_MEMORY_ACCESS_DISCARD;
pub const IREE_HAL_MEMORY_ACCESS_ALL: u16 = 7;
pub const IREE_HAL_BUFFER_COMPATIBILITY_ALLOCATABLE: u32 = 1 << 0;

pub type iree_device_size_t = u64;
pub type iree_hal_buffer_t = c_void;
pub type iree_hal_physical_memory_t = c_void;

/// `iree_hal_buffer_params_t` (32 B, probed): usage u32 @0, access u16 @4,
/// type u32 @8, queue_affinity u64 @16.
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct iree_hal_buffer_params_t {
    pub usage: u32,
    pub access: u16,
    pub _pad0: u16,
    pub type_: u32,
    pub _pad1: u32,
    pub queue_affinity: u64,
}

/// `iree_byte_span_t` = { u8* data; size_t length }.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct iree_byte_span_t {
    pub data: *mut u8,
    pub data_length: iree_host_size_t,
}

/// `iree_hal_buffer_mapping_t` (48 B, probed). We only read `contents.data`
/// after a scoped map; the rest is opaque storage IREE fills. `contents` @0.
#[repr(C, align(8))]
pub struct iree_hal_buffer_mapping_t {
    pub contents: iree_byte_span_t, // 16 B @ 0
    pub _rest: [u8; 32],            // 48 - 16
}
impl iree_hal_buffer_mapping_t {
    pub fn zeroed() -> Self {
        iree_hal_buffer_mapping_t {
            contents: iree_byte_span_t { data: core::ptr::null_mut(), data_length: 0 },
            _rest: [0; 32],
        }
    }
}

/// `iree_string_view_t` = { const char* data; size_t size }.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct iree_string_view_t {
    pub data: *const u8,
    pub size: iree_host_size_t,
}
impl iree_string_view_t {
    /// `iree_make_cstring_view` (inline): {str, strlen(str)}. `s` must be
    /// NUL-terminated; `size` excludes the NUL.
    pub fn cstr(s: &core::ffi::CStr) -> Self {
        let bytes = s.to_bytes();
        iree_string_view_t { data: bytes.as_ptr(), size: bytes.len() }
    }
}

/// `iree_timeout_t` = { iree_timeout_type_t type (i32); iree_time_t nanos (i64) }.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct iree_timeout_t {
    pub type_: i32,
    pub nanos: i64,
}
impl iree_timeout_t {
    /// `iree_infinite_timeout` (inline).
    pub fn infinite() -> Self {
        iree_timeout_t { type_: IREE_TIMEOUT_ABSOLUTE, nanos: IREE_TIME_INFINITE_FUTURE }
    }
}

/// `iree_hal_semaphore_list_t` (24 B). All-zero = empty list.
#[repr(C, align(8))]
#[derive(Clone, Copy)]
pub struct iree_hal_semaphore_list_t {
    pub _bytes: [u8; 24],
}
impl Default for iree_hal_semaphore_list_t {
    fn default() -> Self {
        iree_hal_semaphore_list_t { _bytes: [0; 24] }
    }
}

// --- correctly-sized opaque option/param blobs (filled by C initializers) ---
macro_rules! blob {
    ($name:ident, $size:literal) => {
        #[repr(C, align(8))]
        pub struct $name {
            pub bytes: [u8; $size],
        }
        impl $name {
            pub fn zeroed() -> Self {
                $name { bytes: [0; $size] }
            }
        }
    };
}
blob!(iree_task_topology_t, 22544);
blob!(iree_task_executor_options_t, 40);
blob!(iree_async_proactor_pool_options_t, 80);
blob!(iree_hal_task_device_params_t, 24);
blob!(iree_hal_device_create_params_t, 16);

/// `iree_async_frontier_tracker_options_t` (8 B): { u32 axis_table_capacity;
/// u8 session_epoch; u8 machine_index; pad }. Built to match the inline
/// `..._options_default()` (cap=256, epoch=1, machine=0).
#[repr(C)]
#[derive(Clone, Copy)]
pub struct iree_async_frontier_tracker_options_t {
    pub axis_table_capacity: u32,
    pub session_epoch: u8,
    pub machine_index: u8,
    pub _pad: [u8; 2],
}
impl iree_async_frontier_tracker_options_t {
    pub fn default_opts() -> Self {
        iree_async_frontier_tracker_options_t {
            axis_table_capacity: 256,
            session_epoch: 1,
            machine_index: 0,
            _pad: [0; 2],
        }
    }
}

// Field-offset pokes for the two fields libhrx sets directly.
const OFF_EXEC_WORKER_STACK_SIZE: usize = 24; // iree_task_executor_options_t
const OFF_DEVICE_PARAMS_PROACTOR_POOL: usize = 8; // iree_hal_device_create_params_t

impl iree_task_executor_options_t {
    /// Set `worker_stack_size` (iree_host_size_t @ offset 24).
    pub fn set_worker_stack_size(&mut self, v: usize) {
        unsafe {
            let p = self.bytes.as_mut_ptr().add(OFF_EXEC_WORKER_STACK_SIZE) as *mut usize;
            p.write_unaligned(v);
        }
    }
}
impl iree_hal_device_create_params_t {
    /// Set `proactor_pool` (pointer @ offset 8).
    pub fn set_proactor_pool(&mut self, pool: *mut iree_async_proactor_pool_t) {
        unsafe {
            let p = self.bytes.as_mut_ptr().add(OFF_DEVICE_PARAMS_PROACTOR_POOL)
                as *mut *mut iree_async_proactor_pool_t;
            p.write_unaligned(pool);
        }
    }
}

extern "C" {
    // VM instance + HAL types.
    pub fn iree_vm_instance_create(
        type_capacity: iree_host_size_t,
        allocator: iree_allocator_t,
        out_instance: *mut *mut iree_vm_instance_t,
    ) -> iree_status_t;
    pub fn iree_vm_instance_release(instance: *mut iree_vm_instance_t);
    pub fn iree_hal_module_register_all_types(instance: *mut iree_vm_instance_t) -> iree_status_t;

    // Async proactor pool. options passed BY VALUE (80B blob).
    pub fn iree_async_proactor_pool_options_default() -> iree_async_proactor_pool_options_t;
    pub fn iree_async_proactor_pool_create(
        node_count: u32,
        node_ids: *const u32,
        options: iree_async_proactor_pool_options_t,
        allocator: iree_allocator_t,
        out_pool: *mut *mut iree_async_proactor_pool_t,
    ) -> iree_status_t;
    pub fn iree_async_proactor_pool_release(pool: *mut iree_async_proactor_pool_t);

    // Frontier tracker. options BY VALUE (8B).
    pub fn iree_async_frontier_tracker_create(
        options: iree_async_frontier_tracker_options_t,
        allocator: iree_allocator_t,
        out_tracker: *mut *mut iree_async_frontier_tracker_t,
    ) -> iree_status_t;
    pub fn iree_async_frontier_tracker_release(tracker: *mut iree_async_frontier_tracker_t);

    // Task topology + executor.
    pub fn iree_task_topology_initialize(out_topology: *mut iree_task_topology_t);
    pub fn iree_task_topology_initialize_from_group_count(
        group_count: iree_host_size_t,
        out_topology: *mut iree_task_topology_t,
    );
    pub fn iree_task_topology_deinitialize(topology: *mut iree_task_topology_t);
    pub fn iree_task_executor_options_initialize(out_options: *mut iree_task_executor_options_t);
    pub fn iree_task_executor_create(
        options: iree_task_executor_options_t, // BY VALUE (40B)
        topology: *const iree_task_topology_t,
        allocator: iree_allocator_t,
        out_executor: *mut *mut iree_task_executor_t,
    ) -> iree_status_t;
    pub fn iree_task_executor_release(executor: *mut iree_task_executor_t);

    // Executable loaders.
    pub fn iree_hal_create_all_available_executable_loaders(
        plugin_manager: *mut c_void,
        capacity: iree_host_size_t,
        out_count: *mut iree_host_size_t,
        out_loaders: *mut *mut iree_hal_executable_loader_t,
        allocator: iree_allocator_t,
    ) -> iree_status_t;
    pub fn iree_hal_executable_loader_release(loader: *mut iree_hal_executable_loader_t);

    // HAL allocator (heap).
    pub fn iree_hal_allocator_create_heap(
        identifier: iree_string_view_t,
        data_allocator: iree_allocator_t,
        host_allocator: iree_allocator_t,
        out_allocator: *mut *mut iree_hal_allocator_t,
    ) -> iree_status_t;
    pub fn iree_hal_allocator_retain(allocator: *mut iree_hal_allocator_t);
    pub fn iree_hal_allocator_release(allocator: *mut iree_hal_allocator_t);

    // local-task driver.
    pub fn iree_hal_task_device_params_initialize(out_params: *mut iree_hal_task_device_params_t);
    pub fn iree_hal_task_driver_create(
        identifier: iree_string_view_t,
        default_params: *const iree_hal_task_device_params_t,
        queue_count: iree_host_size_t,
        queue_executors: *const *mut iree_task_executor_t,
        loader_count: iree_host_size_t,
        loaders: *mut *mut iree_hal_executable_loader_t,
        device_allocator: *mut iree_hal_allocator_t,
        host_allocator: iree_allocator_t,
        out_driver: *mut *mut iree_hal_driver_t,
    ) -> iree_status_t;

    // HAL device.
    pub fn iree_hal_driver_create_default_device(
        driver: *mut iree_hal_driver_t,
        create_params: *const iree_hal_device_create_params_t,
        host_allocator: iree_allocator_t,
        out_device: *mut *mut iree_hal_device_t,
    ) -> iree_status_t;
    pub fn iree_hal_driver_release(driver: *mut iree_hal_driver_t);
    pub fn iree_hal_device_allocator(device: *mut iree_hal_device_t) -> *mut iree_hal_allocator_t;
    pub fn iree_hal_device_retain(device: *mut iree_hal_device_t);
    pub fn iree_hal_device_release(device: *mut iree_hal_device_t);
    pub fn iree_hal_device_group_create_from_device(
        device: *mut iree_hal_device_t,
        frontier_tracker: *mut iree_async_frontier_tracker_t,
        host_allocator: iree_allocator_t,
        out_device_group: *mut *mut iree_hal_device_group_t,
    ) -> iree_status_t;
    pub fn iree_hal_device_group_release(device_group: *mut iree_hal_device_group_t);
    pub fn iree_hal_device_wait_semaphores(
        device: *mut iree_hal_device_t,
        wait_mode: i32,
        semaphore_list: iree_hal_semaphore_list_t, // BY VALUE (24B)
        timeout: iree_timeout_t,                   // BY VALUE (16B)
        flags: u32,
    ) -> iree_status_t;
    pub fn iree_hal_device_query_i64(
        device: *mut iree_hal_device_t,
        category: iree_string_view_t,
        key: iree_string_view_t,
        out_value: *mut i64,
    ) -> iree_status_t;

    // --- HAL allocator: buffer allocation/import + compatibility ---
    pub fn iree_hal_allocator_allocate_buffer(
        allocator: *mut iree_hal_allocator_t,
        params: iree_hal_buffer_params_t, // BY VALUE (32B)
        allocation_size: iree_device_size_t,
        out_buffer: *mut *mut iree_hal_buffer_t,
    ) -> iree_status_t;
    pub fn iree_hal_allocator_query_buffer_compatibility(
        allocator: *mut iree_hal_allocator_t,
        params: iree_hal_buffer_params_t,
        allocation_size: iree_device_size_t,
        out_params: *mut iree_hal_buffer_params_t,
        out_allocation_size: *mut iree_device_size_t,
    ) -> u32; // iree_hal_buffer_compatibility_t (bitfield)

    // --- HAL buffer: retain/release/map/unmap ---
    pub fn iree_hal_buffer_retain(buffer: *mut iree_hal_buffer_t);
    pub fn iree_hal_buffer_release(buffer: *mut iree_hal_buffer_t);
    pub fn iree_hal_buffer_map_range(
        buffer: *mut iree_hal_buffer_t,
        mapping_mode: u32,
        memory_access: u16,
        byte_offset: iree_device_size_t,
        byte_length: iree_device_size_t,
        out_mapping: *mut iree_hal_buffer_mapping_t,
    ) -> iree_status_t;
    pub fn iree_hal_buffer_unmap_range(mapping: *mut iree_hal_buffer_mapping_t) -> iree_status_t;

    // --- synchronous transfers ---
    pub fn iree_hal_device_transfer_h2d(
        device: *mut iree_hal_device_t,
        source: *const c_void,
        target: *mut iree_hal_buffer_t,
        target_offset: iree_device_size_t,
        data_length: iree_device_size_t,
        flags: u32,
        timeout: iree_timeout_t,
    ) -> iree_status_t;
    pub fn iree_hal_device_transfer_d2h(
        device: *mut iree_hal_device_t,
        source: *mut iree_hal_buffer_t,
        source_offset: iree_device_size_t,
        target: *mut c_void,
        data_length: iree_device_size_t,
        flags: u32,
        timeout: iree_timeout_t,
    ) -> iree_status_t;
}
