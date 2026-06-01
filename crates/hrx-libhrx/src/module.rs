//! Rust port of libhrx/src/libhrx/module.c — VMFB module load + function
//! lookup/invoke (VM context with HAL + bytecode modules).
#![allow(non_snake_case)]

use core::ffi::{c_char, c_void};
use core::sync::atomic::{AtomicI32, Ordering};

use crate::common::*;
use crate::device::{hrx_device_release, hrx_device_retain, HrxDevice};
use crate::value_list::HrxValueList;
use iree_sys as iree;
use iree_sys::fem;
use iree_sys::init as ireei;

/// `hrx_module_s` = { ref_count, device, bytecode_module, hal_module, context }.
#[repr(C)]
pub struct HrxModuleS {
    pub ref_count: AtomicI32,
    pub device: HrxDevice,
    pub bytecode_module: *mut fem::iree_vm_module_t,
    pub hal_module: *mut fem::iree_vm_module_t,
    pub context: *mut fem::iree_vm_context_t,
}
pub type HrxModule = *mut HrxModuleS;

/// `hrx_function_s` = { ref_count, module, vm_function (16B) }.
#[repr(C)]
pub struct HrxFunctionS {
    pub ref_count: AtomicI32,
    pub module: HrxModule,
    pub vm_function: fem::iree_vm_function_t,
}
pub type HrxFunction = *mut HrxFunctionS;

unsafe fn destroy_partial(m: HrxModule) {
    if !(*m).context.is_null() {
        fem::iree_vm_context_release((*m).context);
    }
    if !(*m).hal_module.is_null() {
        fem::iree_vm_module_release((*m).hal_module);
    }
    if !(*m).bytecode_module.is_null() {
        fem::iree_vm_module_release((*m).bytecode_module);
    }
    if !(*m).device.is_null() {
        hrx_device_release((*m).device);
    }
    libc::free(m as *mut c_void);
}

#[no_mangle]
pub unsafe extern "C" fn hrx_module_load_vmfb(
    device: HrxDevice,
    vmfb_data: *const c_void,
    vmfb_size: usize,
    module: *mut HrxModule,
) -> HrxStatus {
    if device.is_null() || vmfb_data.is_null() || module.is_null() {
        return hrx_make_status(
            HrxStatusCode::InvalidArgument as i32,
            c"device, vmfb_data, or module is NULL".as_ptr(),
        );
    }
    *module = core::ptr::null_mut();
    if vmfb_size == 0 {
        return hrx_make_status(HrxStatusCode::InvalidArgument as i32, c"vmfb_size must be > 0".as_ptr());
    }

    let (vm_instance, initialized) = crate::runtime::shared_vm_instance();
    if !initialized || vm_instance.is_null() {
        return hrx_make_status(HrxStatusCode::Unavailable as i32, c"runtime is not initialized".as_ptr());
    }

    let loaded = libc::calloc(1, core::mem::size_of::<HrxModuleS>()) as *mut HrxModuleS;
    if loaded.is_null() {
        return hrx_make_status(HrxStatusCode::OutOfMemory as i32, c"failed to allocate module".as_ptr());
    }
    (*loaded).ref_count = AtomicI32::new(0); // set to 1 on success (matches C order)

    let alloc = iree::allocator_system();
    let archive = iree::iree_const_byte_span_t {
        data: vmfb_data as *const u8,
        data_length: vmfb_size,
    };
    let s = fem::iree_vm_bytecode_module_create(
        vm_instance,
        fem::IREE_VM_BYTECODE_MODULE_FLAG_NONE,
        archive,
        iree::allocator_null(),
        alloc,
        &mut (*loaded).bytecode_module,
    );
    if !iree::status_is_ok(s) {
        destroy_partial(loaded);
        return hrx_status_from_iree(s);
    }

    let device_group = (*device).hal_device_group;
    if device_group.is_null() {
        destroy_partial(loaded);
        return hrx_make_status(
            HrxStatusCode::FailedPrecondition as i32,
            c"device is missing its HAL device group".as_ptr(),
        );
    }
    fem::iree_hal_device_group_retain(device_group);
    let s = fem::iree_hal_module_create(
        vm_instance,
        fem::iree_hal_module_device_policy_default(),
        device_group,
        fem::IREE_HAL_MODULE_FLAG_NONE,
        fem::iree_hal_module_debug_sink_null(),
        alloc,
        &mut (*loaded).hal_module,
    );
    ireei::iree_hal_device_group_release(device_group);
    if !iree::status_is_ok(s) {
        destroy_partial(loaded);
        return hrx_status_from_iree(s);
    }

    let modules = [(*loaded).hal_module, (*loaded).bytecode_module];
    let s = fem::iree_vm_context_create_with_modules(
        vm_instance,
        fem::IREE_VM_CONTEXT_FLAG_NONE,
        2,
        modules.as_ptr(),
        alloc,
        &mut (*loaded).context,
    );
    if !iree::status_is_ok(s) {
        destroy_partial(loaded);
        return hrx_status_from_iree(s);
    }

    (*loaded).ref_count.store(1, Ordering::Relaxed);
    (*loaded).device = device;
    hrx_device_retain((*loaded).device);
    *module = loaded;
    hrx_ok_status()
}

#[no_mangle]
pub unsafe extern "C" fn hrx_module_retain(module: HrxModule) {
    fem::iree_vm_context_retain((*module).context);
    fem::iree_vm_module_retain((*module).hal_module);
    fem::iree_vm_module_retain((*module).bytecode_module);
    hrx_device_retain((*module).device);
    (*module).ref_count.fetch_add(1, Ordering::Relaxed);
}

#[no_mangle]
pub unsafe extern "C" fn hrx_module_release(module: HrxModule) {
    fem::iree_vm_context_release((*module).context);
    fem::iree_vm_module_release((*module).hal_module);
    fem::iree_vm_module_release((*module).bytecode_module);
    if (*module).ref_count.fetch_sub(1, Ordering::AcqRel) == 1 {
        hrx_device_release((*module).device);
        libc::free(module as *mut c_void);
    } else {
        hrx_device_release((*module).device);
    }
}

#[no_mangle]
pub unsafe extern "C" fn hrx_module_lookup_function(
    module: HrxModule,
    name: *const c_char,
    function: *mut HrxFunction,
) -> HrxStatus {
    if module.is_null() || name.is_null() || function.is_null() {
        return hrx_make_status(
            HrxStatusCode::InvalidArgument as i32,
            c"module, name, or function is NULL".as_ptr(),
        );
    }
    *function = core::ptr::null_mut();
    let resolved = libc::calloc(1, core::mem::size_of::<HrxFunctionS>()) as *mut HrxFunctionS;
    if resolved.is_null() {
        return hrx_make_status(HrxStatusCode::OutOfMemory as i32, c"failed to allocate function".as_ptr());
    }
    (*resolved).vm_function = fem::iree_vm_function_t::zeroed();
    let s = fem::iree_vm_context_resolve_function(
        (*module).context,
        ireei::iree_string_view_t::cstr_raw(name),
        &mut (*resolved).vm_function,
    );
    if !iree::status_is_ok(s) {
        libc::free(resolved as *mut c_void);
        return hrx_status_from_iree(s);
    }
    (*resolved).ref_count = AtomicI32::new(1);
    (*resolved).module = module;
    hrx_module_retain(module);
    *function = resolved;
    hrx_ok_status()
}

#[no_mangle]
pub unsafe extern "C" fn hrx_function_retain(function: HrxFunction) {
    hrx_module_retain((*function).module);
    (*function).ref_count.fetch_add(1, Ordering::Relaxed);
}

#[no_mangle]
pub unsafe extern "C" fn hrx_function_release(function: HrxFunction) {
    hrx_module_release((*function).module);
    if (*function).ref_count.fetch_sub(1, Ordering::AcqRel) == 1 {
        libc::free(function as *mut c_void);
    }
}

#[no_mangle]
pub unsafe extern "C" fn hrx_function_invoke(
    module: HrxModule,
    function: HrxFunction,
    args: HrxValueList,
    rets: HrxValueList,
) -> HrxStatus {
    if module.is_null() || function.is_null() {
        return hrx_make_status(
            HrxStatusCode::InvalidArgument as i32,
            c"module or function is NULL".as_ptr(),
        );
    }
    if (*function).module != module {
        return hrx_make_status(
            HrxStatusCode::InvalidArgument as i32,
            c"function does not belong to module".as_ptr(),
        );
    }
    let alloc = iree::allocator_system();
    let inputs = if args.is_null() { core::ptr::null() } else { (*args).vm_list };
    let outputs = if rets.is_null() { core::ptr::null_mut() } else { (*rets).vm_list };
    hrx_status_from_iree(fem::iree_vm_invoke(
        (*module).context,
        (*function).vm_function,
        fem::IREE_VM_INVOCATION_FLAG_NONE,
        core::ptr::null(),
        inputs,
        outputs,
        alloc,
    ))
}
