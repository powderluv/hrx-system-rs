//! Rust reimplementation of libhrx (the public HRX C ABI), over iree-sys.
//!
//! This crate produces `libhrx_rs.so`, a drop-in for the C `libhrx.so` exporting
//! the same `hrx_*` symbols. Ported incrementally; this batch is the
//! GPU-independent modules: status, host_allocator, value_list (i64/null_ref).
//! Device/stream/buffer modules follow.

mod buffer;
mod buffer_view;
mod common;
mod device;
mod executable;
mod fence;
mod handle;
mod host_allocator;
mod module;
mod pool;
mod queue_ops;
mod runtime;
mod semaphore;
mod status;
mod stream;
mod value_list;

pub use buffer::{HrxBuffer, HrxBufferParams, HrxBufferS};
pub use buffer_view::{HrxBufferView, HrxBufferViewS};
pub use common::{HrxHostAllocator, HrxStatus, HrxStatusCode, HrxStatusS};
pub use queue_ops::{HrxBufferRef, HrxHostCallFn, HrxSemaphoreList};
pub use device::{HrxDevice, HrxDeviceS};
pub use executable::{HrxExecutable, HrxExecutableExportInfo, HrxExecutableS};
pub use fence::{HrxFence, HrxFenceS};
pub use module::{HrxFunction, HrxFunctionS, HrxModule, HrxModuleS};
pub use semaphore::{HrxSemaphore, HrxSemaphoreS};
pub use stream::{HrxStream, HrxStreamS, HrxTimelinePoint};
pub use value_list::HrxValueListS;
