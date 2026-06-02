//! Rust port of libhrx/src/libhrx/module.c — VMFB module load + function
//! lookup/invoke (VM context with HAL + bytecode modules).
//!
//! Phase-2 owned model: `hrx_module_t` and `hrx_function_t` are `Arc` data
//! pointers. retain/release are `Arc` refcount ops. The module owns one reference
//! each to its VM context, HAL module, bytecode module, and device, released once
//! on the last drop via ordered field drop — `context` → `hal_module` →
//! `bytecode_module` → `device`, matching the C release order. There is no
//! explicit `Drop`, and creation builds the RAII wrappers as it goes so an
//! error-path return releases exactly what was created so far (replacing the C
//! `destroy_partial` ladder and the two-phase `ref_count = 0 then 1` dance — there
//! is simply no `Arc` until every field succeeds). A function holds one module
//! reference (`ModuleRef`) for its lifetime; the per-retain module fanout collapses
//! to one ref held while the function is alive, which is observably equivalent to
//! C's balanced per-call accounting (the module is freed at the same moment).
#![allow(non_snake_case)]

use core::ffi::{c_char, c_void};

use crate::common::*;
use crate::device::{DeviceRef, HrxDevice};
use crate::handle::{handle_ref, handle_release, handle_retain, into_handle};
use crate::value_list::{value_list_vm, HrxValueList};
use iree_hal::{HalVmContext, HalVmModule};
use iree_sys as iree;
use iree_sys::fem;
use iree_sys::init as ireei;

/// `hrx_module_s` — the object behind the opaque `hrx_module_t`. Declaration order
/// is load-bearing for drop: `context` → `hal_module` → `bytecode_module` →
/// `device`, matching the C release order.
pub struct HrxModuleS {
    context: HalVmContext,
    // The next three are held only for their RAII drop (release after `context`,
    // in C order); never read directly.
    #[allow(dead_code)]
    hal_module: HalVmModule,
    #[allow(dead_code)]
    bytecode_module: HalVmModule,
    #[allow(dead_code)]
    device: DeviceRef,
}
pub type HrxModule = *mut HrxModuleS;

/// `hrx_function_s` — the object behind the opaque `hrx_function_t`. Holds one
/// module reference (`ModuleRef`, released on drop) plus the resolved VM function,
/// a plain 16-byte value (module ptr + linkage + ordinal — not reference-counted).
pub struct HrxFunctionS {
    module: ModuleRef,
    vm_function: fem::iree_vm_function_t,
}
pub type HrxFunction = *mut HrxFunctionS;

/// An owned reference to a `hrx_module_t`: construction retains the module, `Drop`
/// releases it. Lets a function hold exactly one module reference for its lifetime
/// via RAII (analogous to `DeviceRef`).
pub(crate) struct ModuleRef(HrxModule);

impl ModuleRef {
    /// Retain `module` and take an owned reference.
    ///
    /// # Safety
    /// `module` must be a live `hrx_module_t`.
    pub(crate) unsafe fn retain(module: HrxModule) -> Self {
        hrx_module_retain(module);
        Self(module)
    }
    pub(crate) fn as_ptr(&self) -> HrxModule {
        self.0
    }
}

impl Drop for ModuleRef {
    fn drop(&mut self) {
        // SAFETY: we hold one reference taken in `retain`; release it once.
        unsafe { hrx_module_release(self.0) };
    }
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

    let alloc = iree::allocator_system();
    let archive = iree::iree_const_byte_span_t {
        data: vmfb_data as *const u8,
        data_length: vmfb_size,
    };
    let mut raw_bytecode: *mut fem::iree_vm_module_t = core::ptr::null_mut();
    let s = fem::iree_vm_bytecode_module_create(
        vm_instance,
        fem::IREE_VM_BYTECODE_MODULE_FLAG_NONE,
        archive,
        iree::allocator_null(),
        alloc,
        &mut raw_bytecode,
    );
    if !iree::status_is_ok(s) {
        return hrx_status_from_iree(s);
    }
    // From here on, an early return drops `bytecode_module` (and later `hal_module`)
    // in reverse declaration order, releasing exactly what the C `destroy_partial`
    // ladder would have released, in the same order.
    let bytecode_module = HalVmModule::from_owned(raw_bytecode).expect("created bytecode module is non-null");

    let device_group = (*device).hal_device_group.as_ptr();
    if device_group.is_null() {
        return hrx_make_status(
            HrxStatusCode::FailedPrecondition as i32,
            c"device is missing its HAL device group".as_ptr(),
        );
    }
    fem::iree_hal_device_group_retain(device_group);
    let mut raw_hal: *mut fem::iree_vm_module_t = core::ptr::null_mut();
    let s = fem::iree_hal_module_create(
        vm_instance,
        fem::iree_hal_module_device_policy_default(),
        device_group,
        fem::IREE_HAL_MODULE_FLAG_NONE,
        fem::iree_hal_module_debug_sink_null(),
        alloc,
        &mut raw_hal,
    );
    ireei::iree_hal_device_group_release(device_group);
    if !iree::status_is_ok(s) {
        return hrx_status_from_iree(s);
    }
    let hal_module = HalVmModule::from_owned(raw_hal).expect("created HAL module is non-null");

    let modules = [hal_module.as_ptr(), bytecode_module.as_ptr()];
    let mut raw_context: *mut fem::iree_vm_context_t = core::ptr::null_mut();
    let s = fem::iree_vm_context_create_with_modules(
        vm_instance,
        fem::IREE_VM_CONTEXT_FLAG_NONE,
        2,
        modules.as_ptr(),
        alloc,
        &mut raw_context,
    );
    if !iree::status_is_ok(s) {
        return hrx_status_from_iree(s);
    }
    let context = HalVmContext::from_owned(raw_context).expect("created context is non-null");

    let device = DeviceRef::retain(device);
    *module = into_handle(HrxModuleS { context, hal_module, bytecode_module, device });
    hrx_ok_status()
}

#[no_mangle]
pub unsafe extern "C" fn hrx_module_retain(module: HrxModule) {
    handle_retain(module);
}

#[no_mangle]
pub unsafe extern "C" fn hrx_module_release(module: HrxModule) {
    // The HAL teardown (release context/hal_module/bytecode_module/device) moved
    // into the field drops, which run on the last reference in C order. C released
    // those on every call to balance per-retain retains; the owned model holds one
    // reference each and releases them once on drop — observably equivalent.
    handle_release(module);
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
    let mut vm_function = fem::iree_vm_function_t::zeroed();
    let s = fem::iree_vm_context_resolve_function(
        handle_ref(module).context.as_ptr(),
        ireei::iree_string_view_t::cstr_raw(name),
        &mut vm_function,
    );
    if !iree::status_is_ok(s) {
        return hrx_status_from_iree(s);
    }
    let module_ref = ModuleRef::retain(module);
    *function = into_handle(HrxFunctionS { module: module_ref, vm_function });
    hrx_ok_status()
}

#[no_mangle]
pub unsafe extern "C" fn hrx_function_retain(function: HrxFunction) {
    handle_retain(function);
}

#[no_mangle]
pub unsafe extern "C" fn hrx_function_release(function: HrxFunction) {
    // The module reference (ModuleRef) is released by the field drop on the last
    // reference — equivalent to C's per-call hrx_module_release.
    handle_release(function);
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
    let func = handle_ref(function);
    if func.module.as_ptr() != module {
        return hrx_make_status(
            HrxStatusCode::InvalidArgument as i32,
            c"function does not belong to module".as_ptr(),
        );
    }
    let alloc = iree::allocator_system();
    let inputs = if args.is_null() { core::ptr::null() } else { value_list_vm(args) };
    let outputs = if rets.is_null() { core::ptr::null_mut() } else { value_list_vm(rets) };
    hrx_status_from_iree(fem::iree_vm_invoke(
        handle_ref(module).context.as_ptr(),
        func.vm_function,
        fem::IREE_VM_INVOCATION_FLAG_NONE,
        core::ptr::null(),
        inputs,
        outputs,
        alloc,
    ))
}
