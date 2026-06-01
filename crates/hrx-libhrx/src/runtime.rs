//! Rust port of libhrx/src/libhrx/runtime.c (shared state + CPU accelerator) and
//! device.c (device ops). Mirrors the C global-singleton model.
//!
//! The GPU accelerator path (hrx_gpu_*) needs the IREE amdgpu HAL driver
//! registration symbols, which live in a separate archive set; it is deferred.
//! The CPU/local-task path is fully ported here and is what the MI300 CPU oracle
//! exercises (and is GPU-host-independent).
#![allow(non_snake_case)]

use core::ffi::{c_char, c_int, c_void};
use core::sync::atomic::{AtomicI32, Ordering};
use std::sync::Mutex;

use crate::common::*;
use crate::device::*;
use iree_sys as iree;
use iree_sys::init as ireei;

pub const HRX_MAX_DEVICES: usize = 64;

// Version (matches hrx_runtime.h HRX_VERSION_*).
const HRX_VERSION_MAJOR: c_int = 0;
const HRX_VERSION_MINOR: c_int = 1;
const HRX_VERSION_PATCH: c_int = 0;

/// Shared infrastructure created on first accelerator init.
pub struct SharedState {
    pub vm_instance: *mut iree::iree_vm_instance_t,
    pub proactor_pool: *mut iree::iree_async_proactor_pool_t,
    pub host_allocator: iree::iree_allocator_t,
    pub init_count: i32,
    pub initialized: bool,
}
unsafe impl Send for SharedState {}

pub struct CpuState {
    /// Boxed device slots; the C uses a fixed array but callers hold raw
    /// pointers into it, so the storage must be stable. We box each device.
    pub devices: Vec<*mut HrxDeviceS>,
    pub device_count: i32,
    pub initialized: bool,
    pub driver: *mut iree::iree_hal_driver_t,
}
unsafe impl Send for CpuState {}

struct Globals {
    shared: SharedState,
    cpu: CpuState,
}
unsafe impl Send for Globals {}

static G: Mutex<Globals> = Mutex::new(Globals {
    shared: SharedState {
        vm_instance: core::ptr::null_mut(),
        proactor_pool: core::ptr::null_mut(),
        host_allocator: iree::iree_allocator_t {
            self_: core::ptr::null_mut(),
            ctl: core::ptr::null_mut(),
        },
        init_count: 0,
        initialized: false,
    },
    cpu: CpuState {
        devices: Vec::new(),
        device_count: 0,
        initialized: false,
        driver: core::ptr::null_mut(),
    },
});

#[no_mangle]
pub extern "C" fn hrx_runtime_version(major: *mut c_int, minor: *mut c_int, patch: *mut c_int) {
    unsafe {
        if !major.is_null() {
            *major = HRX_VERSION_MAJOR;
        }
        if !minor.is_null() {
            *minor = HRX_VERSION_MINOR;
        }
        if !patch.is_null() {
            *patch = HRX_VERSION_PATCH;
        }
    }
}

/// Snapshot of the shared VM instance pointer + initialized flag, for modules
/// that read `hrx_get_shared_state()->vm_instance` (e.g. module.rs). Returns
/// (vm_instance, initialized) under the global lock.
pub(crate) fn shared_vm_instance() -> (*mut iree::iree_vm_instance_t, bool) {
    let g = G.lock().unwrap();
    (g.shared.vm_instance, g.shared.initialized)
}

/// Mirror of hrx_ensure_shared_state (idempotent; bumps init_count).
fn ensure_shared_state(g: &mut Globals) -> HrxStatus {
    if g.shared.initialized {
        g.shared.init_count += 1;
        return hrx_ok_status();
    }
    g.shared.host_allocator = iree::allocator_system();
    unsafe {
        let mut inst: *mut iree::iree_vm_instance_t = core::ptr::null_mut();
        let s = ireei::iree_vm_instance_create(
            iree::IREE_VM_TYPE_CAPACITY_DEFAULT,
            g.shared.host_allocator,
            &mut inst,
        );
        if !iree::status_is_ok(s) {
            return hrx_status_from_iree(s);
        }
        let s = ireei::iree_hal_module_register_all_types(inst);
        if !iree::status_is_ok(s) {
            ireei::iree_vm_instance_release(inst);
            return hrx_status_from_iree(s);
        }
        g.shared.vm_instance = inst;

        // Proactor pool for async I/O (required by local-task devices).
        let mut node_id: u32 = 0;
        let mut pool: *mut iree::iree_async_proactor_pool_t = core::ptr::null_mut();
        let opts = ireei::iree_async_proactor_pool_options_default();
        let s = ireei::iree_async_proactor_pool_create(
            1,
            &mut node_id,
            opts,
            g.shared.host_allocator,
            &mut pool,
        );
        if !iree::status_is_ok(s) {
            ireei::iree_vm_instance_release(inst);
            g.shared.vm_instance = core::ptr::null_mut();
            return hrx_status_from_iree(s);
        }
        g.shared.proactor_pool = pool;
    }
    g.shared.initialized = true;
    g.shared.init_count = 1;
    hrx_ok_status()
}

fn release_shared_state(g: &mut Globals) {
    if !g.shared.initialized {
        return;
    }
    g.shared.init_count -= 1;
    if g.shared.init_count > 0 {
        return;
    }
    unsafe {
        if !g.shared.proactor_pool.is_null() {
            ireei::iree_async_proactor_pool_release(g.shared.proactor_pool);
            g.shared.proactor_pool = core::ptr::null_mut();
        }
        if !g.shared.vm_instance.is_null() {
            ireei::iree_vm_instance_release(g.shared.vm_instance);
            g.shared.vm_instance = core::ptr::null_mut();
        }
    }
    g.shared.initialized = false;
}

/// Single-device group via a frontier tracker (mirror of
/// hrx_create_single_device_group). Returns the IREE status.
unsafe fn create_single_device_group(
    device: *mut iree::iree_hal_device_t,
    host_allocator: iree::iree_allocator_t,
    out_group: *mut *mut iree::iree_hal_device_group_t,
) -> iree::iree_status_t {
    *out_group = core::ptr::null_mut();
    let mut tracker: *mut iree::iree_async_frontier_tracker_t = core::ptr::null_mut();
    let s = ireei::iree_async_frontier_tracker_create(
        ireei::iree_async_frontier_tracker_options_t::default_opts(),
        host_allocator,
        &mut tracker,
    );
    if !iree::status_is_ok(s) {
        return s;
    }
    let s = ireei::iree_hal_device_group_create_from_device(
        device, tracker, host_allocator, out_group,
    );
    ireei::iree_async_frontier_tracker_release(tracker);
    s
}

/// Build a local-task HAL device (mirror of hrx_create_local_task_device).
/// Returns (driver, hal_device) on success.
unsafe fn create_local_task_device(
    group_count: usize,
    host_allocator: iree::iree_allocator_t,
) -> Result<(*mut iree::iree_hal_driver_t, *mut iree::iree_hal_device_t), HrxStatus> {
    // Topology (22544 bytes — heap-box it).
    let mut topology = Box::new(ireei::iree_task_topology_t::zeroed());
    ireei::iree_task_topology_initialize(&mut *topology);
    ireei::iree_task_topology_initialize_from_group_count(group_count, &mut *topology);

    let mut exec_options = ireei::iree_task_executor_options_t::zeroed();
    ireei::iree_task_executor_options_initialize(&mut exec_options);
    exec_options.set_worker_stack_size(256 * 1024);

    let mut executor: *mut iree::iree_task_executor_t = core::ptr::null_mut();
    let s = ireei::iree_task_executor_create(exec_options, &*topology, host_allocator, &mut executor);
    ireei::iree_task_topology_deinitialize(&mut *topology);
    if !iree::status_is_ok(s) {
        return Err(hrx_status_from_iree(s));
    }

    // Executable loaders.
    let mut loaders: [*mut iree::iree_hal_executable_loader_t; 8] = [core::ptr::null_mut(); 8];
    let mut loader_count: usize = 0;
    let s = ireei::iree_hal_create_all_available_executable_loaders(
        core::ptr::null_mut(),
        loaders.len(),
        &mut loader_count,
        loaders.as_mut_ptr(),
        host_allocator,
    );
    if !iree::status_is_ok(s) {
        ireei::iree_task_executor_release(executor);
        return Err(hrx_status_from_iree(s));
    }

    // Heap allocator for host-accessible buffers.
    let mut device_allocator: *mut iree::iree_hal_allocator_t = core::ptr::null_mut();
    let s = ireei::iree_hal_allocator_create_heap(
        ireei::iree_string_view_t::cstr(c"hrx"),
        host_allocator,
        host_allocator,
        &mut device_allocator,
    );
    if !iree::status_is_ok(s) {
        for i in 0..loader_count {
            ireei::iree_hal_executable_loader_release(loaders[i]);
        }
        ireei::iree_task_executor_release(executor);
        return Err(hrx_status_from_iree(s));
    }

    // Assemble the local-task driver.
    let mut task_params = ireei::iree_hal_task_device_params_t::zeroed();
    ireei::iree_hal_task_device_params_initialize(&mut task_params);

    let mut driver: *mut iree::iree_hal_driver_t = core::ptr::null_mut();
    let queue_executors = [executor];
    let s = ireei::iree_hal_task_driver_create(
        ireei::iree_string_view_t::cstr(c"local-task"),
        &task_params,
        1,
        queue_executors.as_ptr(),
        loader_count,
        loaders.as_mut_ptr(),
        device_allocator,
        host_allocator,
        &mut driver,
    );
    // Driver took ownership references; release ours.
    ireei::iree_task_executor_release(executor);
    for i in 0..loader_count {
        ireei::iree_hal_executable_loader_release(loaders[i]);
    }
    ireei::iree_hal_allocator_release(device_allocator);
    if !iree::status_is_ok(s) {
        return Err(hrx_status_from_iree(s));
    }

    // Create device from driver, providing the proactor pool.
    let mut device_params = ireei::iree_hal_device_create_params_t::zeroed();
    // proactor_pool is set by the caller (needs g.shared.proactor_pool).
    // We thread it through via a thread-unsafe-but-locked global read below.
    device_params.set_proactor_pool(POOL_FOR_DEVICE.load(Ordering::Relaxed));

    let mut hal_device: *mut iree::iree_hal_device_t = core::ptr::null_mut();
    let s = ireei::iree_hal_driver_create_default_device(
        driver,
        &device_params,
        host_allocator,
        &mut hal_device,
    );
    if !iree::status_is_ok(s) {
        ireei::iree_hal_driver_release(driver);
        return Err(hrx_status_from_iree(s));
    }
    Ok((driver, hal_device))
}

// The proactor pool pointer needed inside create_local_task_device; set under
// the G lock before the call.
static POOL_FOR_DEVICE: core::sync::atomic::AtomicPtr<iree::iree_async_proactor_pool_t> =
    core::sync::atomic::AtomicPtr::new(core::ptr::null_mut());

#[no_mangle]
pub extern "C" fn hrx_cpu_initialize(_flags: u32) -> HrxStatus {
    let mut g = G.lock().unwrap();
    if g.cpu.initialized {
        return hrx_make_status(
            HrxStatusCode::AlreadyExists as i32,
            c"CPU accelerator already initialized".as_ptr(),
        );
    }
    let s = ensure_shared_state(&mut g);
    if !hrx_status_is_ok(s) {
        return s;
    }
    let host_allocator = g.shared.host_allocator;
    POOL_FOR_DEVICE.store(g.shared.proactor_pool, Ordering::Relaxed);

    unsafe {
        let (driver, hal_device) = match create_local_task_device(4, host_allocator) {
            Ok(v) => v,
            Err(e) => {
                release_shared_state(&mut g);
                return e;
            }
        };

        let mut device_group: *mut iree::iree_hal_device_group_t = core::ptr::null_mut();
        let s = create_single_device_group(hal_device, host_allocator, &mut device_group);
        if !iree::status_is_ok(s) {
            ireei::iree_hal_device_release(hal_device);
            ireei::iree_hal_driver_release(driver);
            release_shared_state(&mut g);
            return hrx_status_from_iree(s);
        }

        // Build the device (boxed for stable address; callers hold the pointer).
        let hal_alloc = ireei::iree_hal_device_allocator(hal_device);
        ireei::iree_hal_allocator_retain(hal_alloc);
        let dev = Box::into_raw(Box::new(HrxDeviceS {
            ref_count: AtomicI32::new(1),
            type_: HRX_ACCELERATOR_CPU,
            ordinal: 0,
            hal_device,
            hal_device_group: device_group,
            allocator: HrxAllocatorInline {
                ref_count: AtomicI32::new(1),
                hal_allocator: hal_alloc,
                device: core::ptr::null_mut(), // set just below
            },
            name: cstr_array::<128>("CPU 0 (local-task)"),
            architecture: cstr_array::<64>("host"),
        }));
        // dev->allocator.device = dev (back-pointer used by hrx_allocator_*).
        (*dev).allocator.device = dev;

        g.cpu.devices.clear();
        g.cpu.devices.push(dev);
        g.cpu.driver = driver;
        g.cpu.device_count = 1;
        g.cpu.initialized = true;
    }
    hrx_ok_status()
}

#[no_mangle]
pub extern "C" fn hrx_cpu_shutdown() -> HrxStatus {
    let mut g = G.lock().unwrap();
    if !g.cpu.initialized {
        return hrx_make_status(
            HrxStatusCode::InvalidArgument as i32,
            c"CPU accelerator not initialized".as_ptr(),
        );
    }
    unsafe {
        let devices = core::mem::take(&mut g.cpu.devices);
        for d in devices {
            hrx_device_release(d);
        }
        if !g.cpu.driver.is_null() {
            ireei::iree_hal_driver_release(g.cpu.driver);
            g.cpu.driver = core::ptr::null_mut();
        }
    }
    g.cpu.device_count = 0;
    g.cpu.initialized = false;
    release_shared_state(&mut g);
    hrx_ok_status()
}

#[no_mangle]
pub unsafe extern "C" fn hrx_cpu_device_count(count: *mut c_int) -> HrxStatus {
    if count.is_null() {
        return hrx_make_status(HrxStatusCode::InvalidArgument as i32, c"count is NULL".as_ptr());
    }
    let g = G.lock().unwrap();
    if !g.cpu.initialized {
        return hrx_make_status(
            HrxStatusCode::Unavailable as i32,
            c"CPU accelerator not initialized".as_ptr(),
        );
    }
    *count = g.cpu.device_count;
    hrx_ok_status()
}

#[no_mangle]
pub unsafe extern "C" fn hrx_cpu_device_get(index: c_int, device: *mut HrxDevice) -> HrxStatus {
    if device.is_null() {
        return hrx_make_status(HrxStatusCode::InvalidArgument as i32, c"device is NULL".as_ptr());
    }
    let g = G.lock().unwrap();
    if !g.cpu.initialized {
        return hrx_make_status(
            HrxStatusCode::Unavailable as i32,
            c"CPU accelerator not initialized".as_ptr(),
        );
    }
    if index < 0 || index >= g.cpu.device_count {
        return hrx_make_status(
            HrxStatusCode::OutOfRange as i32,
            c"CPU device index out of range".as_ptr(),
        );
    }
    *device = g.cpu.devices[index as usize];
    hrx_ok_status()
}

/// Copy a &str into a fixed [c_char; N] NUL-terminated array (mirrors snprintf).
fn cstr_array<const N: usize>(s: &str) -> [c_char; N] {
    let mut out = [0 as c_char; N];
    let bytes = s.as_bytes();
    let n = bytes.len().min(N - 1);
    for i in 0..n {
        out[i] = bytes[i] as c_char;
    }
    out
}
