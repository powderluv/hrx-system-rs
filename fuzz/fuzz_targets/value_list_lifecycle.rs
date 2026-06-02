#![no_main]
//! Stateful lifecycle fuzzer for `hrx_value_list_t`, run under libfuzzer + ASAN.
//!
//! It drives random sequences of create / push_i64 / push_null_ref / size /
//! get_i64 / retain / release over a small pool of lists. `value_list` is
//! GPU-free (it needs only the CPU runtime, initialized once), so this exercises
//! the owned-`Arc` value_list object and the `Arc` handle machinery it shares with
//! every other migrated object, with random interleavings ASAN watches.
//!
//! The harness tracks its own reference count per slot and releases exactly that
//! many at the end of each input, so it keeps perfect balance: if the library
//! frees too early a later op is a use-after-free (ASAN aborts), and if it fails to
//! free, ASAN's leak check (when enabled) flags it. Either way a refcount bug in
//! `handle_retain`/`handle_release`/the `HalVmList` drop surfaces as a crash.
//!
//! We bind the public C ABI directly (treating the fuzzer as a C consumer of
//! libhrx) rather than via Rust paths; depending on the port links its
//! instrumented rlib (coverage + ASAN reach into the port), and `extern crate`
//! forces that link so the `#[no_mangle]` symbols below resolve. The package is
//! `hrx-libhrx`; its lib crate is named `hrx_rs`.
extern crate hrx_rs as _;

use core::ffi::c_void;
use libfuzzer_sys::fuzz_target;
use std::sync::Once;

// Opaque handles — all pointer-sized; we never deref them, only pass them back.
type HrxStatus = *mut c_void;
type HrxValueList = *mut c_void;

extern "C" {
    fn hrx_cpu_initialize(flags: u32) -> HrxStatus;
    fn hrx_status_ignore(status: HrxStatus);
    fn hrx_value_list_create(capacity: usize, list: *mut HrxValueList) -> HrxStatus;
    fn hrx_value_list_push_i64(list: HrxValueList, value: i64) -> HrxStatus;
    fn hrx_value_list_push_null_ref(list: HrxValueList) -> HrxStatus;
    fn hrx_value_list_size(list: HrxValueList, size: *mut usize) -> HrxStatus;
    fn hrx_value_list_get_i64(list: HrxValueList, index: usize, value: *mut i64) -> HrxStatus;
    fn hrx_value_list_retain(list: HrxValueList);
    fn hrx_value_list_release(list: HrxValueList);
}

static INIT: Once = Once::new();
const POOL: usize = 4;

fn take8(data: &[u8], i: &mut usize) -> i64 {
    let mut b = [0u8; 8];
    for slot in b.iter_mut() {
        *slot = data.get(*i).copied().unwrap_or(0);
        *i += 1;
    }
    i64::from_le_bytes(b)
}

fuzz_target!(|data: &[u8]| {
    // value_list needs the CPU runtime (VM instance); init once, reuse.
    INIT.call_once(|| unsafe { hrx_status_ignore(hrx_cpu_initialize(0)) });

    let mut lists: [HrxValueList; POOL] = [core::ptr::null_mut(); POOL];
    // How many references the harness currently holds for each slot.
    let mut refs: [u32; POOL] = [0; POOL];

    let mut i = 0usize;
    while i < data.len() {
        let byte = data[i];
        i += 1;
        let slot = ((byte >> 4) as usize) % POOL; // high nibble selects a list
        let live = !lists[slot].is_null();
        match byte % 6 {
            0 if !live => {
                let cap = data.get(i).copied().unwrap_or(0) as usize;
                i += 1;
                let mut l: HrxValueList = core::ptr::null_mut();
                unsafe { hrx_status_ignore(hrx_value_list_create(cap, &mut l)) };
                if !l.is_null() {
                    lists[slot] = l;
                    refs[slot] = 1;
                }
            }
            1 if live => {
                let v = take8(data, &mut i);
                unsafe { hrx_status_ignore(hrx_value_list_push_i64(lists[slot], v)) };
            }
            2 if live => unsafe {
                hrx_status_ignore(hrx_value_list_push_null_ref(lists[slot]))
            },
            3 if live => {
                let mut sz = 0usize;
                unsafe { hrx_status_ignore(hrx_value_list_size(lists[slot], &mut sz)) };
                let idx = data.get(i).copied().unwrap_or(0) as usize;
                i += 1;
                let mut out = 0i64;
                unsafe { hrx_status_ignore(hrx_value_list_get_i64(lists[slot], idx, &mut out)) };
            }
            4 if live => {
                // Extra reference; balanced at cleanup.
                unsafe { hrx_value_list_retain(lists[slot]) };
                refs[slot] += 1;
            }
            5 if live => {
                // Drop one reference; clear the slot only when the last one goes.
                unsafe { hrx_value_list_release(lists[slot]) };
                refs[slot] -= 1;
                if refs[slot] == 0 {
                    lists[slot] = core::ptr::null_mut();
                }
            }
            _ => {}
        }
    }

    // Release every outstanding reference so balance is exact — a missing library
    // free shows up as an ASAN leak, a premature free as a use-after-free above.
    for slot in 0..POOL {
        while refs[slot] > 0 {
            unsafe { hrx_value_list_release(lists[slot]) };
            refs[slot] -= 1;
        }
    }
});
