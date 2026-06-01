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
    /// `iree_make_cstring_view` for a raw NUL-terminated C pointer (or null).
    pub unsafe fn cstr_raw(p: *const core::ffi::c_char) -> Self {
        if p.is_null() {
            iree_string_view_t { data: core::ptr::null(), size: 0 }
        } else {
            iree_string_view_t { data: p as *const u8, size: libc::strlen(p) }
        }
    }
}

/// `iree_timeout_t` = { iree_timeout_type_t type (i32); iree_time_t nanos (i64) }.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct iree_timeout_t {
    pub type_: i32,
    pub nanos: i64,
}
pub const IREE_TIME_INFINITE_PAST: i64 = i64::MIN;
pub const IREE_TIMEOUT_RELATIVE: i32 = 1;

impl iree_timeout_t {
    /// `iree_infinite_timeout` (inline).
    pub fn infinite() -> Self {
        iree_timeout_t { type_: IREE_TIMEOUT_ABSOLUTE, nanos: IREE_TIME_INFINITE_FUTURE }
    }
    /// `iree_immediate_timeout` (inline) = {ABSOLUTE, INFINITE_PAST}.
    pub fn immediate() -> Self {
        iree_timeout_t { type_: IREE_TIMEOUT_ABSOLUTE, nanos: IREE_TIME_INFINITE_PAST }
    }
    /// `iree_make_timeout_ns` (inline) = {RELATIVE, ns}.
    pub fn relative_ns(ns: i64) -> Self {
        iree_timeout_t { type_: IREE_TIMEOUT_RELATIVE, nanos: ns }
    }
}

/// `iree_hal_semaphore_list_t` (24 B): { count usize @0, semaphores ptr @8,
/// payload_values ptr @16 }. All-zero = empty list.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct iree_hal_semaphore_list_t {
    pub count: iree_host_size_t,
    pub semaphores: *mut *mut iree_hal_semaphore_t,
    pub payload_values: *mut u64,
}
impl Default for iree_hal_semaphore_list_t {
    fn default() -> Self {
        iree_hal_semaphore_list_t {
            count: 0,
            semaphores: core::ptr::null_mut(),
            payload_values: core::ptr::null_mut(),
        }
    }
}

/// `iree_hal_buffer_ref_t` (32B): first word is a u32 bitfield (reserved:8 +
/// buffer_slot:24, both 0 here), then buffer ptr @8, offset @16, length @24.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct iree_hal_buffer_ref_t {
    pub reserved_and_slot: u32,
    pub _pad: u32,
    pub buffer: *mut iree_hal_buffer_t,
    pub offset: iree_device_size_t,
    pub length: iree_device_size_t,
}
impl iree_hal_buffer_ref_t {
    /// `iree_hal_make_buffer_ref` (inline).
    pub fn make(buffer: *mut iree_hal_buffer_t, offset: u64, length: u64) -> Self {
        iree_hal_buffer_ref_t {
            reserved_and_slot: 0,
            _pad: 0,
            buffer,
            offset,
            length,
        }
    }
}

/// `iree_hal_dispatch_config_t` (64B, align 8): workgroup_size[3]@0,
/// workgroup_count[3]@12, workgroup_count_ref (buffer_ref 32B)@24,
/// dynamic_workgroup_local_memory u32@56, tail pad to 64.
#[repr(C, align(8))]
#[derive(Clone, Copy)]
pub struct iree_hal_dispatch_config_t {
    pub workgroup_size: [u32; 3],
    pub workgroup_count: [u32; 3],
    pub workgroup_count_ref: iree_hal_buffer_ref_t,
    pub dynamic_workgroup_local_memory: u32,
    pub _pad: u32,
}
impl iree_hal_dispatch_config_t {
    /// Static dispatch: only workgroup_size/count populated, the rest zeroed
    /// (matches the libhrx designated-initializer, which leaves the indirect
    /// fields zero).
    pub fn new_static(size: [u32; 3], count: [u32; 3]) -> Self {
        iree_hal_dispatch_config_t {
            workgroup_size: size,
            workgroup_count: count,
            workgroup_count_ref: iree_hal_buffer_ref_t::make(core::ptr::null_mut(), 0, 0),
            dynamic_workgroup_local_memory: 0,
            _pad: 0,
        }
    }
}

/// `iree_hal_buffer_ref_list_t` (16B): { count: iree_host_size_t, values: *const ref }.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct iree_hal_buffer_ref_list_t {
    pub count: iree_host_size_t,
    pub values: *const iree_hal_buffer_ref_t,
}

/// `iree_hal_dispatch_flags_t` = uint64_t.
pub type iree_hal_dispatch_flags_t = u64;

/// `iree_hal_memory_barrier_t` (8B): { source_scope u32 @0, target_scope u32 @4 }.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct iree_hal_memory_barrier_t {
    pub source_scope: u32,
    pub target_scope: u32,
}

/// `iree_hal_buffer_binding_table_t` (16B). All-zero = empty.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct iree_hal_buffer_binding_table_t {
    pub count: iree_host_size_t,
    pub bindings: *const iree_hal_buffer_ref_t,
}
impl Default for iree_hal_buffer_binding_table_t {
    fn default() -> Self {
        iree_hal_buffer_binding_table_t { count: 0, bindings: core::ptr::null() }
    }
}

// Opaque handles for the stream/semaphore path.
pub type iree_hal_semaphore_t = c_void;
pub type iree_hal_command_buffer_t = c_void;

// Command-buffer / queue enum values (probed).
pub const IREE_HAL_SEMAPHORE_FLAG_NONE: u32 = 0;
pub const IREE_HAL_COMMAND_BUFFER_MODE_ONE_SHOT: u32 = 1;
pub const IREE_HAL_COMMAND_CATEGORY_TRANSFER: u32 = 1;
pub const IREE_HAL_COMMAND_CATEGORY_DISPATCH: u32 = 2;
pub const IREE_HAL_EXECUTION_STAGE_COMMAND_RETIRE: u32 = 16;
pub const IREE_HAL_EXECUTION_STAGE_COMMAND_ISSUE: u32 = 1;
pub const IREE_HAL_EXECUTION_BARRIER_FLAG_NONE: u32 = 0;
pub const IREE_HAL_QUEUE_AFFINITY_ANY: u64 = u64::MAX;

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

    // --- timeline semaphore ---
    pub fn iree_hal_semaphore_create(
        device: *mut iree_hal_device_t,
        queue_affinity: u64,
        initial_value: u64,
        flags: u32,
        out_semaphore: *mut *mut iree_hal_semaphore_t,
    ) -> iree_status_t;
    pub fn iree_hal_semaphore_retain(semaphore: *mut iree_hal_semaphore_t);
    pub fn iree_hal_semaphore_release(semaphore: *mut iree_hal_semaphore_t);
    pub fn iree_hal_semaphore_query(
        semaphore: *mut iree_hal_semaphore_t,
        out_value: *mut u64,
    ) -> iree_status_t;
    pub fn iree_hal_semaphore_wait(
        semaphore: *mut iree_hal_semaphore_t,
        value: u64,
        timeout: iree_timeout_t,
        flags: u32,
    ) -> iree_status_t;
    pub fn iree_hal_semaphore_signal(
        semaphore: *mut iree_hal_semaphore_t,
        new_value: u64,
        frontier: *mut c_void,
    ) -> iree_status_t;

    // --- command buffer ---
    pub fn iree_hal_command_buffer_create(
        device: *mut iree_hal_device_t,
        mode: u32,
        command_categories: u32,
        queue_affinity: u64,
        binding_capacity: iree_host_size_t,
        out_command_buffer: *mut *mut iree_hal_command_buffer_t,
    ) -> iree_status_t;
    pub fn iree_hal_command_buffer_begin(cb: *mut iree_hal_command_buffer_t) -> iree_status_t;
    pub fn iree_hal_command_buffer_end(cb: *mut iree_hal_command_buffer_t) -> iree_status_t;
    pub fn iree_hal_command_buffer_release(cb: *mut iree_hal_command_buffer_t);
    pub fn iree_hal_command_buffer_execution_barrier(
        cb: *mut iree_hal_command_buffer_t,
        source_stage_mask: u32,
        target_stage_mask: u32,
        flags: u32,
        memory_barrier_count: iree_host_size_t,
        memory_barriers: *const iree_hal_memory_barrier_t,
        buffer_barrier_count: iree_host_size_t,
        buffer_barriers: *const c_void,
    ) -> iree_status_t;
    pub fn iree_hal_command_buffer_fill_buffer(
        cb: *mut iree_hal_command_buffer_t,
        target_ref: iree_hal_buffer_ref_t,
        pattern: *const c_void,
        pattern_length: iree_host_size_t,
        flags: u32,
    ) -> iree_status_t;
    pub fn iree_hal_command_buffer_copy_buffer(
        cb: *mut iree_hal_command_buffer_t,
        source_ref: iree_hal_buffer_ref_t,
        target_ref: iree_hal_buffer_ref_t,
        flags: u32,
    ) -> iree_status_t;
    pub fn iree_hal_command_buffer_update_buffer(
        cb: *mut iree_hal_command_buffer_t,
        source_buffer: *const c_void,
        source_offset: iree_host_size_t,
        target_ref: iree_hal_buffer_ref_t,
        flags: u32,
    ) -> iree_status_t;

    // --- device queue ---
    pub fn iree_hal_device_queue_execute(
        device: *mut iree_hal_device_t,
        queue_affinity: u64,
        wait_semaphore_list: iree_hal_semaphore_list_t,
        signal_semaphore_list: iree_hal_semaphore_list_t,
        command_buffer: *mut iree_hal_command_buffer_t,
        binding_table: iree_hal_buffer_binding_table_t,
        flags: u32,
    ) -> iree_status_t;
    pub fn iree_hal_device_queue_barrier(
        device: *mut iree_hal_device_t,
        queue_affinity: u64,
        wait_semaphore_list: iree_hal_semaphore_list_t,
        signal_semaphore_list: iree_hal_semaphore_list_t,
        flags: u32,
    ) -> iree_status_t;
    pub fn iree_hal_device_queue_host_call(
        device: *mut iree_hal_device_t,
        queue_affinity: u64,
        wait_semaphore_list: iree_hal_semaphore_list_t,
        signal_semaphore_list: iree_hal_semaphore_list_t,
        call: iree_hal_host_call_t,
        args: *const u64, // const uint64_t args[4]
        flags: u64,
    ) -> iree_status_t;

    // --- dispatch (command buffer + device queue) ---
    pub fn iree_hal_command_buffer_dispatch(
        cb: *mut iree_hal_command_buffer_t,
        executable: *mut super::fem::iree_hal_executable_t,
        function: super::fem::iree_hal_executable_function_t, // BY VALUE (8B)
        config: iree_hal_dispatch_config_t,                   // BY VALUE (64B)
        constants: super::iree_const_byte_span_t,             // BY VALUE (16B)
        bindings: iree_hal_buffer_ref_list_t,                 // BY VALUE (16B)
        flags: iree_hal_dispatch_flags_t,
    ) -> iree_status_t;
    pub fn iree_hal_device_queue_dispatch(
        device: *mut iree_hal_device_t,
        queue_affinity: u64,
        wait_semaphore_list: iree_hal_semaphore_list_t,
        signal_semaphore_list: iree_hal_semaphore_list_t,
        executable: *mut super::fem::iree_hal_executable_t,
        function: super::fem::iree_hal_executable_function_t,
        config: iree_hal_dispatch_config_t,
        constants: super::iree_const_byte_span_t,
        bindings: iree_hal_buffer_ref_list_t,
        flags: iree_hal_dispatch_flags_t,
    ) -> iree_status_t;
}

/// `iree_hal_host_call_t` (16B): { fn, user_data }. The fn is
/// `fn(user_data, args[4], context) -> iree_status_t`.
pub type iree_hal_host_call_fn_t =
    Option<unsafe extern "C" fn(*mut c_void, *const u64, *mut c_void) -> iree_status_t>;
#[repr(C)]
#[derive(Clone, Copy)]
pub struct iree_hal_host_call_t {
    pub fn_: iree_hal_host_call_fn_t,
    pub user_data: *mut c_void,
}
pub const IREE_HAL_HOST_CALL_FLAG_NONE: u64 = 0;

/// `iree_hal_device_info_t` (40B): { device_id u64 @0, path sv @8, name sv @24 }.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct iree_hal_device_info_t {
    pub device_id: u64,
    pub path: iree_string_view_t,
    pub name: iree_string_view_t,
}

extern "C" {
    // amdgpu driver registration + driver registry + GPU device enumeration.
    pub fn iree_hal_driver_registry_default() -> *mut c_void;
    pub fn iree_hal_amdgpu_driver_module_register(registry: *mut c_void) -> iree_status_t;
    pub fn iree_hal_driver_registry_try_create(
        registry: *mut c_void,
        driver_name: iree_string_view_t,
        host_allocator: iree_allocator_t,
        out_driver: *mut *mut iree_hal_driver_t,
    ) -> iree_status_t;
    pub fn iree_hal_driver_query_available_devices(
        driver: *mut iree_hal_driver_t,
        host_allocator: iree_allocator_t,
        out_device_info_count: *mut iree_host_size_t,
        out_device_infos: *mut *mut iree_hal_device_info_t,
    ) -> iree_status_t;
    pub fn iree_hal_driver_create_device_by_ordinal(
        driver: *mut iree_hal_driver_t,
        device_ordinal: iree_host_size_t,
        param_count: iree_host_size_t,
        params: *const c_void, // const iree_string_pair_t* (NULL here)
        create_params: *const iree_hal_device_create_params_t,
        host_allocator: iree_allocator_t,
        out_device: *mut *mut iree_hal_device_t,
    ) -> iree_status_t;
    // (iree_hal_driver_release + iree_hal_device_query_i64 declared earlier.)
}
