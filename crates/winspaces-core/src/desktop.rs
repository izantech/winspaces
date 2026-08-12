//! Virtual desktop tracking: per-monitor spaces, window eligibility, and the
//! show/hide state machine. See the submodules for the split rationale.

mod eligibility;
mod index_math;
mod manager;
mod monitor;
mod state;
mod visibility;

pub use eligibility::{is_live_window, is_valid_window};
pub use index_math::remap_index_after_reorder;
pub use manager::DesktopManager;
pub use monitor::{MonitorState, MAX_MONITORS};
pub use visibility::{reclaim_orphaned_windows, set_window_visibility};
