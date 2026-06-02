#![no_main]
//! Stateful lifecycle fuzzer for `hrx_buffer_t` on a CPU device, under libfuzzer +
//! ASAN. This is the most intricate owned object: it has a `RefCell` map-state
//! (`Unmapped`/`Mapped`), a `Drop` that unmaps if still mapped, an `Option<HalPool>`,
//! a `DeviceRef`, and the shared `Arc` handle machinery. The fuzzer drives random
//! allocate / retain / release / map / unmap / get_size sequences over a pool of
//! buffers and, on a successful map, touches the mapped memory (so a bad mapping
//! pointer/length is an ASAN out-of-bounds). It deliberately releases buffers that
//! are still mapped, exercising the Drop-unmap path.
//!
//! GPU-free: it uses the local-task CPU device + host-visible memory, so it runs
//! anywhere the IREE archives link. Per-slot refcount tracking keeps the harness
//! balanced, so a premature library free → ASAN use-after-free and a missing free
//! → ASAN leak.
extern crate hrx_rs as _;

use core::ffi::{c_int, c_void};
use libfuzzer_sys::fuzz_target;
use std::sync::Once;

type HrxStatus = *mut c_void;
type HrxDevice = *mut c_void;
type HrxAllocator = *mut c_void;
type HrxBuffer = *mut c_void;

/// `hrx_buffer_params_t` (24 B): type @0, access @4 (u16), usage @8, affinity @16.
#[repr(C)]
#[derive(Clone, Copy)]
struct HrxBufferParams {
    type_: u32,
    access: u16,
    _pad0: u16,
    usage: u32,
    _pad1: u32,
    queue_affinity: u64,
}

extern "C" {
    fn hrx_cpu_initialize(flags: u32) -> HrxStatus;
    fn hrx_cpu_device_get(index: c_int, device: *mut HrxDevice) -> HrxStatus;
    fn hrx_device_allocator(device: HrxDevice) -> HrxAllocator;
    fn hrx_status_ignore(status: HrxStatus);
    fn hrx_allocator_allocate_buffer(
        allocator: HrxAllocator,
        params: HrxBufferParams,
        size: usize,
        buffer: *mut HrxBuffer,
    ) -> HrxStatus;
    fn hrx_buffer_retain(buffer: HrxBuffer);
    fn hrx_buffer_release(buffer: HrxBuffer);
    fn hrx_buffer_map(
        buffer: HrxBuffer,
        flags: u32,
        offset: usize,
        size: usize,
        mapped: *mut *mut c_void,
    ) -> HrxStatus;
    fn hrx_buffer_unmap(buffer: HrxBuffer) -> HrxStatus;
    fn hrx_buffer_get_size(buffer: HrxBuffer, size: *mut usize) -> HrxStatus;
}

// Host-local + host-visible, mappable — the allocatable/mappable params the mem
// differential uses, so the fuzzer reaches the success paths, not just validation.
const HOST_PARAMS: HrxBufferParams = HrxBufferParams {
    type_: 0x46,            // HRX_MEMORY_TYPE_HOST_LOCAL | HOST_VISIBLE
    access: 7,              // HRX_MEMORY_ACCESS_ALL
    _pad0: 0,
    usage: 0x0100_0C03,     // HRX_BUFFER_USAGE_DEFAULT | MAPPING_SCOPED
    _pad1: 0,
    queue_affinity: 0,
};
const HRX_MAP_READ: u32 = 1;
const HRX_MAP_WRITE: u32 = 2;

static INIT: Once = Once::new();
static mut ALLOC: HrxAllocator = core::ptr::null_mut();
const POOL: usize = 4;

fn setup_allocator() -> HrxAllocator {
    INIT.call_once(|| unsafe {
        hrx_status_ignore(hrx_cpu_initialize(0));
        let mut dev: HrxDevice = core::ptr::null_mut();
        hrx_status_ignore(hrx_cpu_device_get(0, &mut dev));
        // device_allocator returns a borrowed pointer into the device; the device
        // lives for the process, so caching it is fine.
        ALLOC = hrx_device_allocator(dev);
    });
    unsafe { ALLOC }
}

fuzz_target!(|data: &[u8]| {
    let alloc = setup_allocator();
    if alloc.is_null() {
        return; // CPU runtime unavailable; nothing to fuzz
    }

    let mut bufs: [HrxBuffer; POOL] = [core::ptr::null_mut(); POOL];
    let mut refs: [u32; POOL] = [0; POOL];
    let mut sizes: [usize; POOL] = [0; POOL];

    let mut i = 0usize;
    while i < data.len() {
        let byte = data[i];
        i += 1;
        let slot = ((byte >> 4) as usize) % POOL;
        let live = !bufs[slot].is_null();
        match byte % 6 {
            0 if !live => {
                // Size 1..=4096, derived from the next byte.
                let size = (data.get(i).copied().unwrap_or(0) as usize) * 16 + 1;
                i += 1;
                let mut b: HrxBuffer = core::ptr::null_mut();
                let st = unsafe { hrx_allocator_allocate_buffer(alloc, HOST_PARAMS, size, &mut b) };
                unsafe { hrx_status_ignore(st) };
                if !b.is_null() {
                    bufs[slot] = b;
                    refs[slot] = 1;
                    sizes[slot] = size;
                }
            }
            1 if live => {
                unsafe { hrx_buffer_retain(bufs[slot]) };
                refs[slot] += 1;
            }
            2 if live => {
                // Release one ref — may free a still-mapped buffer, exercising the
                // Drop-unmap path.
                unsafe { hrx_buffer_release(bufs[slot]) };
                refs[slot] -= 1;
                if refs[slot] == 0 {
                    bufs[slot] = core::ptr::null_mut();
                }
            }
            3 if live => {
                let mut ptr: *mut c_void = core::ptr::null_mut();
                let st = unsafe {
                    hrx_buffer_map(bufs[slot], HRX_MAP_READ | HRX_MAP_WRITE, 0, sizes[slot], &mut ptr)
                };
                let ok = st.is_null();
                unsafe { hrx_status_ignore(st) };
                // On a successful map, touch the first byte: a bad mapping pointer
                // or length is then an ASAN out-of-bounds / invalid write.
                if ok && !ptr.is_null() {
                    unsafe { (ptr as *mut u8).write_volatile(byte) };
                }
            }
            4 if live => unsafe { hrx_status_ignore(hrx_buffer_unmap(bufs[slot])) },
            5 if live => {
                let mut sz = 0usize;
                unsafe { hrx_status_ignore(hrx_buffer_get_size(bufs[slot], &mut sz)) };
            }
            _ => {}
        }
    }

    // Release everything still live (Drop unmaps any still-mapped buffer).
    for slot in 0..POOL {
        while refs[slot] > 0 {
            unsafe { hrx_buffer_release(bufs[slot]) };
            refs[slot] -= 1;
        }
    }
});
