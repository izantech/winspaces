//! Notification hook for scheduling asynchronous retile passes: the same
//! dependency inversion as `spaces::notify`, so core never names the bin.

use std::sync::OnceLock;

static RETILE_SCHEDULER: OnceLock<fn()> = OnceLock::new();

/// Install the bin-level retile scheduler. First call wins, matching
/// `OnceLock` semantics; the daemon installs exactly once at startup.
pub fn set_retile_scheduler(scheduler: fn()) {
    let _ = RETILE_SCHEDULER.set(scheduler);
}

/// Request an asynchronous retile pass. Producers call this whenever
/// tiling-managed state or window membership changes.
pub fn schedule_retile() {
    if let Some(scheduler) = RETILE_SCHEDULER.get() {
        scheduler();
    }
}
