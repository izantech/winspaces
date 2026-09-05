//! Space tracking: per-monitor spaces, window eligibility, and the
//! show/hide state machine. See the submodules for the split rationale.

mod activation;
mod count_ops;
mod eligibility;
mod enforce;
mod index_math;
mod manager;
mod monitor;
mod notify;
mod rehome;
mod rules;
mod state;
mod sticky;
mod visibility;

pub use activation::ActivationDecision;
pub use eligibility::{is_framed_window, is_live_window, is_tileable_window, is_valid_window};
pub use enforce::RestoreTarget;
pub use index_math::remap_index_after_reorder;
pub use manager::SpaceManager;

pub use monitor::{MonitorState, MAX_MONITORS};
pub use notify::{set_switch_observer, SwitchNotice};
pub use rehome::RehomeTrigger;
pub(crate) use visibility::AnimationGuard;
pub use visibility::{reclaim_orphaned_windows, set_window_visibility};
