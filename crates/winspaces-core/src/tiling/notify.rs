//! Notification hook for scheduling asynchronous retile passes.
//!
//! Follows the same dependency-inversion pattern as `spaces::notify`.

use std::sync::atomic::{AtomicPtr, Ordering};

static RETILE_SCHEDULER: AtomicPtr<()> = AtomicPtr::new(std::ptr::null_mut());

/// Install the bin-level retile scheduler callback.
pub fn set_retile_scheduler(scheduler: fn()) {
    RETILE_SCHEDULER.store(scheduler as *mut (), Ordering::Release);
}

/// Request an asynchronous retile pass. Producers call this whenever
/// tiling-managed state or window membership changes.
pub fn schedule_retile() {
    let ptr = RETILE_SCHEDULER.load(Ordering::Acquire);
    if !ptr.is_null() {
        let func: fn() = unsafe { std::mem::transmute(ptr) };
        func();
    }
}
