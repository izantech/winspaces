//! Workspace-rule capture, matching, and placement: mapping windows onto
//! saved rules so they land back where the user put them.

mod capture;
mod dump;
pub(crate) mod identity;
mod matching;
mod placement;
mod query;

pub use capture::{capture_active_workspace, capture_active_workspace_detailed, CapturedWindow};
pub use dump::dump_all_window_metrics;
pub use matching::{match_float_rule_for_window, match_rule_for_window, score_rule};

pub use placement::apply_rule_to_window;
pub use query::{
    get_process_image_path, get_window_aumid, get_window_class, get_window_placement_info,
    get_window_title,
};
