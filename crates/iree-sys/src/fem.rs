//! FFI for the fence / executable / VM-module surface libhrx's fence.c,
//! executable.c, module.c use. By-value struct sizes probed against the real
//! headers:
//!   iree_hal_executable_params_t      = 64 B
//!   iree_hal_executable_function_info_t = 48 B (name sv@0, flags u32@16,
//!       constant_count u16@24, binding_count u16@26, parameter_count u16@28,
//!       workgroup_size[3] u32@32)
//!   iree_hal_module_device_policy_t   = 16 B  (filled by *_default())
//!   iree_hal_module_debug_sink_t      = 32 B  (filled by *_null())
//!   iree_vm_function_t                = 16 B  (filled by resolve_function)
//!   iree_hal_executable_function_t    = 8 B   ({ value: u64 })

use super::init::iree_string_view_t;
use super::iree_const_byte_span_t;
use super::*;
use core::ffi::c_void;

pub type iree_hal_fence_t = c_void;
pub type iree_hal_executable_t = c_void;
pub type iree_hal_executable_cache_t = c_void;
pub type iree_vm_module_t = c_void;
pub type iree_vm_context_t = c_void;
pub type iree_hal_buffer_view_t = c_void;

pub type iree_hal_dim_t = u64;
pub type iree_hal_element_type_t = u32;
pub type iree_hal_encoding_type_t = u32;
pub const IREE_HAL_ENCODING_TYPE_DENSE_ROW_MAJOR: u32 = 1;

/// `iree_hal_executable_function_t` (8B): a single u64 handle value.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct iree_hal_executable_function_t {
    pub value: u64,
}

/// `iree_vm_function_t` (16B) — opaque to us (filled by resolve_function and
/// passed by value to iree_vm_invoke). Model as a 16-byte blob.
#[repr(C, align(8))]
#[derive(Clone, Copy)]
pub struct iree_vm_function_t {
    pub bytes: [u8; 16],
}
impl iree_vm_function_t {
    pub fn zeroed() -> Self {
        iree_vm_function_t { bytes: [0; 16] }
    }
}

/// `iree_hal_module_device_policy_t` (16B) — filled by *_default(), passed by
/// value to iree_hal_module_create.
#[repr(C, align(8))]
#[derive(Clone, Copy)]
pub struct iree_hal_module_device_policy_t {
    pub bytes: [u8; 16],
}

/// `iree_hal_module_debug_sink_t` (32B) — filled by *_null(), by value.
#[repr(C, align(8))]
#[derive(Clone, Copy)]
pub struct iree_hal_module_debug_sink_t {
    pub bytes: [u8; 32],
}

/// `iree_hal_executable_params_t` (64B) — filled by *_initialize() then a couple
/// fields set. We poke executable_format (string_view @ offset), executable_data
/// (const_byte_span), caching_mode. Offsets probed below.
#[repr(C, align(8))]
pub struct iree_hal_executable_params_t {
    pub bytes: [u8; 64],
}
impl iree_hal_executable_params_t {
    pub fn zeroed() -> Self {
        iree_hal_executable_params_t { bytes: [0; 64] }
    }
    /// Set caching_mode (u32 @8).
    pub fn set_caching_mode(&mut self, v: u32) {
        unsafe { (self.bytes.as_mut_ptr().add(8) as *mut u32).write_unaligned(v) }
    }
    /// Set executable_format (iree_string_view_t @16: data ptr, size).
    pub fn set_executable_format(&mut self, sv: iree_string_view_t) {
        unsafe { (self.bytes.as_mut_ptr().add(16) as *mut iree_string_view_t).write_unaligned(sv) }
    }
    /// Set executable_data (iree_const_byte_span_t @32: data ptr, len).
    pub fn set_executable_data(&mut self, span: iree_const_byte_span_t) {
        unsafe { (self.bytes.as_mut_ptr().add(32) as *mut iree_const_byte_span_t).write_unaligned(span) }
    }
}

/// `iree_hal_executable_function_info_t` (48B). We read flags + counts +
/// workgroup_size after a successful info query.
#[repr(C, align(8))]
pub struct iree_hal_executable_function_info_t {
    pub name: iree_string_view_t, // 16B @ 0
    pub flags: u32,               // @ 16
    pub _pad0: u32,
    pub constant_count: u16, // @ 24
    pub binding_count: u16,  // @ 26
    pub parameter_count: u16, // @ 28
    pub _pad1: u16,
    pub workgroup_size: [u32; 3], // @ 32
    pub _tail: u32,               // 48 - 44
}
impl iree_hal_executable_function_info_t {
    pub fn zeroed() -> Self {
        iree_hal_executable_function_info_t {
            name: iree_string_view_t { data: core::ptr::null(), size: 0 },
            flags: 0,
            _pad0: 0,
            constant_count: 0,
            binding_count: 0,
            parameter_count: 0,
            _pad1: 0,
            workgroup_size: [0; 3],
            _tail: 0,
        }
    }
}

pub const IREE_VM_BYTECODE_MODULE_FLAG_NONE: u32 = 0;
pub const IREE_HAL_MODULE_FLAG_NONE: u32 = 0;
pub const IREE_VM_CONTEXT_FLAG_NONE: u32 = 0;
pub const IREE_VM_INVOCATION_FLAG_NONE: u32 = 0;

extern "C" {
    // --- fence ---
    pub fn iree_hal_fence_create(
        capacity: iree_host_size_t,
        host_allocator: iree_allocator_t,
        out_fence: *mut *mut iree_hal_fence_t,
    ) -> iree_status_t;
    pub fn iree_hal_fence_create_at(
        semaphore: *mut super::init::iree_hal_semaphore_t,
        value: u64,
        host_allocator: iree_allocator_t,
        out_fence: *mut *mut iree_hal_fence_t,
    ) -> iree_status_t;
    // fence retain+release are replaced by the Miri mock shims below (cfg(miri)).
    #[cfg(not(miri))]
    pub fn iree_hal_fence_retain(fence: *mut iree_hal_fence_t);
    #[cfg(not(miri))]
    pub fn iree_hal_fence_release(fence: *mut iree_hal_fence_t);
    pub fn iree_hal_fence_insert(
        fence: *mut iree_hal_fence_t,
        semaphore: *mut super::init::iree_hal_semaphore_t,
        value: u64,
    ) -> iree_status_t;
    pub fn iree_hal_fence_extend(
        into_fence: *mut iree_hal_fence_t,
        from_fence: *mut iree_hal_fence_t,
    ) -> iree_status_t;
    pub fn iree_hal_fence_signal(fence: *mut iree_hal_fence_t) -> iree_status_t;
    pub fn iree_hal_fence_wait(
        fence: *mut iree_hal_fence_t,
        timeout: super::init::iree_timeout_t,
        flags: u32,
    ) -> iree_status_t;

    // --- executable cache + executable ---
    pub fn iree_hal_executable_cache_create(
        device: *mut iree_hal_device_t,
        identifier: iree_string_view_t,
        out_cache: *mut *mut iree_hal_executable_cache_t,
    ) -> iree_status_t;
    pub fn iree_hal_executable_cache_retain(cache: *mut iree_hal_executable_cache_t);
    pub fn iree_hal_executable_cache_release(cache: *mut iree_hal_executable_cache_t);
    pub fn iree_hal_executable_cache_prepare_executable(
        cache: *mut iree_hal_executable_cache_t,
        params: *const iree_hal_executable_params_t,
        out_executable: *mut *mut iree_hal_executable_t,
    ) -> iree_status_t;
    pub fn iree_hal_executable_params_initialize(out_params: *mut iree_hal_executable_params_t);
    pub fn iree_hal_executable_retain(executable: *mut iree_hal_executable_t);
    pub fn iree_hal_executable_release(executable: *mut iree_hal_executable_t);
    pub fn iree_hal_executable_function_count(executable: *mut iree_hal_executable_t) -> iree_host_size_t;
    pub fn iree_hal_executable_function_info(
        executable: *mut iree_hal_executable_t,
        function: iree_hal_executable_function_t,
        out_info: *mut iree_hal_executable_function_info_t,
    ) -> iree_status_t;
    pub fn iree_hal_executable_lookup_function_by_name(
        executable: *mut iree_hal_executable_t,
        name: iree_string_view_t,
        out_function: *mut iree_hal_executable_function_t,
    ) -> iree_status_t;

    // --- VM module / context / invoke ---
    pub fn iree_vm_bytecode_module_create(
        instance: *mut iree_vm_instance_t,
        flags: u32,
        archive_contents: iree_const_byte_span_t,
        archive_allocator: iree_allocator_t,
        allocator: iree_allocator_t,
        out_module: *mut *mut iree_vm_module_t,
    ) -> iree_status_t;
    pub fn iree_hal_module_device_policy_default() -> iree_hal_module_device_policy_t;
    pub fn iree_hal_module_debug_sink_null() -> iree_hal_module_debug_sink_t;
    pub fn iree_hal_module_create(
        instance: *mut iree_vm_instance_t,
        device_policy: iree_hal_module_device_policy_t,
        device_group: *mut iree_hal_device_group_t,
        flags: u32,
        debug_sink: iree_hal_module_debug_sink_t,
        host_allocator: iree_allocator_t,
        out_module: *mut *mut iree_vm_module_t,
    ) -> iree_status_t;
    pub fn iree_vm_module_retain(module: *mut iree_vm_module_t);
    pub fn iree_vm_module_release(module: *mut iree_vm_module_t);
    pub fn iree_hal_device_group_retain(device_group: *mut iree_hal_device_group_t);
    pub fn iree_vm_context_create_with_modules(
        instance: *mut iree_vm_instance_t,
        flags: u32,
        module_count: iree_host_size_t,
        modules: *const *mut iree_vm_module_t,
        allocator: iree_allocator_t,
        out_context: *mut *mut iree_vm_context_t,
    ) -> iree_status_t;
    pub fn iree_vm_context_retain(context: *mut iree_vm_context_t);
    pub fn iree_vm_context_release(context: *mut iree_vm_context_t);
    pub fn iree_vm_context_resolve_function(
        context: *mut iree_vm_context_t,
        full_name: iree_string_view_t,
        out_function: *mut iree_vm_function_t,
    ) -> iree_status_t;
    pub fn iree_vm_invoke(
        context: *mut iree_vm_context_t,
        function: iree_vm_function_t,
        flags: u32,
        policy: *const c_void,
        inputs: *const iree_vm_list_t,
        outputs: *mut iree_vm_list_t,
        host_allocator: iree_allocator_t,
    ) -> iree_status_t;

    // --- buffer view ---
    pub fn iree_hal_buffer_view_create(
        buffer: *mut super::init::iree_hal_buffer_t,
        shape_rank: iree_host_size_t,
        shape: *const iree_hal_dim_t,
        element_type: iree_hal_element_type_t,
        encoding_type: iree_hal_encoding_type_t,
        host_allocator: iree_allocator_t,
        out_buffer_view: *mut *mut iree_hal_buffer_view_t,
    ) -> iree_status_t;
    pub fn iree_hal_buffer_view_retain(buffer_view: *mut iree_hal_buffer_view_t);
    pub fn iree_hal_buffer_view_release(buffer_view: *mut iree_hal_buffer_view_t);
    pub fn iree_hal_buffer_view_shape_rank(buffer_view: *mut iree_hal_buffer_view_t) -> iree_host_size_t;
    pub fn iree_hal_buffer_view_shape_dim(
        buffer_view: *mut iree_hal_buffer_view_t,
        index: iree_host_size_t,
    ) -> iree_hal_dim_t;

    // --- vm ref adapters (return iree_vm_ref_t by value, 16B) ---
    pub fn iree_hal_buffer_retain_ref(buffer: *mut super::init::iree_hal_buffer_t) -> iree_vm_ref_t;
    pub fn iree_hal_buffer_view_retain_ref(buffer_view: *mut iree_hal_buffer_view_t) -> iree_vm_ref_t;
    pub fn iree_hal_fence_retain_ref(fence: *mut iree_hal_fence_t) -> iree_vm_ref_t;

    // --- allocator: import + virtual/physical memory ---
    pub fn iree_hal_allocator_import_buffer(
        allocator: *mut iree_hal_allocator_t,
        params: super::init::iree_hal_buffer_params_t, // BY VALUE (32B)
        external_buffer: *mut iree_hal_external_buffer_t,
        release_callback: iree_hal_buffer_release_callback_t, // BY VALUE (16B)
        out_buffer: *mut *mut super::init::iree_hal_buffer_t,
    ) -> iree_status_t;
    pub fn iree_hal_allocator_supports_virtual_memory(allocator: *mut iree_hal_allocator_t) -> bool;
    pub fn iree_hal_allocator_virtual_memory_query_granularity(
        allocator: *mut iree_hal_allocator_t,
        params: super::init::iree_hal_buffer_params_t,
        out_minimum_page_size: *mut u64,
        out_recommended_page_size: *mut u64,
    ) -> iree_status_t;
    pub fn iree_hal_allocator_virtual_memory_reserve(
        allocator: *mut iree_hal_allocator_t,
        queue_affinity: u64,
        size: u64,
        out_virtual_buffer: *mut *mut super::init::iree_hal_buffer_t,
    ) -> iree_status_t;
    pub fn iree_hal_allocator_virtual_memory_release(
        allocator: *mut iree_hal_allocator_t,
        virtual_buffer: *mut super::init::iree_hal_buffer_t,
    ) -> iree_status_t;
    pub fn iree_hal_allocator_physical_memory_allocate(
        allocator: *mut iree_hal_allocator_t,
        params: super::init::iree_hal_buffer_params_t,
        size: u64,
        host_allocator: iree_allocator_t,
        out_physical_memory: *mut *mut super::init::iree_hal_physical_memory_t,
    ) -> iree_status_t;
    pub fn iree_hal_allocator_physical_memory_free(
        allocator: *mut iree_hal_allocator_t,
        physical_memory: *mut super::init::iree_hal_physical_memory_t,
    ) -> iree_status_t;
    pub fn iree_hal_allocator_virtual_memory_map(
        allocator: *mut iree_hal_allocator_t,
        virtual_buffer: *mut super::init::iree_hal_buffer_t,
        virtual_offset: u64,
        physical_memory: *mut super::init::iree_hal_physical_memory_t,
        physical_offset: u64,
        size: u64,
    ) -> iree_status_t;
    pub fn iree_hal_allocator_virtual_memory_unmap(
        allocator: *mut iree_hal_allocator_t,
        virtual_buffer: *mut super::init::iree_hal_buffer_t,
        virtual_offset: u64,
        size: u64,
    ) -> iree_status_t;
    pub fn iree_hal_allocator_virtual_memory_protect(
        allocator: *mut iree_hal_allocator_t,
        virtual_buffer: *mut super::init::iree_hal_buffer_t,
        virtual_offset: u64,
        size: u64,
        queue_affinity: u64,
        protection: u32,
    ) -> iree_status_t;
}

// Miri shims for fence retain/release; see the matching note in `init`.
#[cfg(miri)]
pub unsafe extern "C" fn iree_hal_fence_retain(fence: *mut iree_hal_fence_t) {
    unsafe { crate::mock::retain(fence as *mut core::ffi::c_void) }
}
#[cfg(miri)]
pub unsafe extern "C" fn iree_hal_fence_release(fence: *mut iree_hal_fence_t) {
    unsafe { crate::mock::release(fence as *mut core::ffi::c_void) }
}

pub const IREE_HAL_EXTERNAL_BUFFER_TYPE_HOST_ALLOCATION: u32 = 1;

/// `iree_hal_external_buffer_t` (24B): type u32@0, flags u32@4, size u64@8,
/// handle union (ptr) @16.
#[repr(C, align(8))]
pub struct iree_hal_external_buffer_t {
    pub type_: u32,
    pub flags: u32,
    pub size: u64,
    pub handle_ptr: *mut c_void, // host_allocation.ptr (union, first member)
}

/// `iree_hal_buffer_release_callback_t` (16B): { fn, user_data }. Null = all-zero.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct iree_hal_buffer_release_callback_t {
    pub fn_: *mut c_void,
    pub user_data: *mut c_void,
}
impl iree_hal_buffer_release_callback_t {
    /// `iree_hal_buffer_release_callback_null()` (inline) — zeroed.
    pub fn null() -> Self {
        iree_hal_buffer_release_callback_t {
            fn_: core::ptr::null_mut(),
            user_data: core::ptr::null_mut(),
        }
    }
}
