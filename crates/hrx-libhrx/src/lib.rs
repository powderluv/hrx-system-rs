//! Rust reimplementation of libhrx (the public HRX C ABI), over iree-sys.
//!
//! This crate produces `libhrx_rs.so`, a drop-in for the C `libhrx.so` exporting
//! the same `hrx_*` symbols. Ported incrementally; this batch is the
//! GPU-independent modules: status, host_allocator, value_list (i64/null_ref).
//! Device/stream/buffer modules follow.

mod common;
mod device;
mod host_allocator;
mod runtime;
mod status;
mod value_list;

pub use common::{HrxHostAllocator, HrxStatus, HrxStatusCode, HrxStatusS};
pub use device::{HrxDevice, HrxDeviceS};
pub use value_list::HrxValueListS;
