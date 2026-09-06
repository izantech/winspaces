//! Workspace-rule placement: the one path every trigger (shell hook,
//! activation, show event, restore) uses to put a window where a
//! `WorkspaceRule` says it belongs.

use windows_sys::Win32::Foundation::HWND;
use winspaces_common::{log_info, WorkspaceRule};

use super::manager::SpaceManager;
use crate::workspaces;

impl SpaceManager {
    /// Apply `rule` to `hwnd`: move it onto the rule's display and saved
    /// geometry, track it on the rule's space, and pin it when the rule says
    /// so. Does not switch spaces; a caller that wants the window on screen
    /// does that itself.
    ///
    /// Aims at the monitor the rule *names*, not at whatever currently covers
    /// the saved coordinates: after a topology change those coordinates can
    /// point at a different display entirely.
    ///
    /// `hwnd` is an opaque handle only forwarded to Win32, which tolerates a
    /// stale one by failing gracefully (see `track_window`).
    #[allow(clippy::not_unsafe_ptr_arg_deref)]
    pub fn place_by_rule(&mut self, hwnd: HWND, rule: &WorkspaceRule) {
        let target = self.monitors.get(rule.display_index).map(|m| m.hmon);
        unsafe { workspaces::apply_rule_to_window(hwnd, rule, target) };
        self.track_window(hwnd, rule.display_index, rule.space_index);
        if rule.is_sticky {
            self.set_sticky(hwnd, true);
        }
    }

    /// Match `hwnd` against `rules`; on a hit, log it as `what`, place the
    /// window and switch to its space with it activated. Returns whether a
    /// rule matched. `what` keeps each trigger's log line distinguishable
    /// ("ShellHook auto-placing", "EVENT_OBJECT_SHOW auto-placing", ...).
    #[allow(clippy::not_unsafe_ptr_arg_deref)]
    pub fn try_place_by_rule(&mut self, hwnd: HWND, rules: &[WorkspaceRule], what: &str) -> bool {
        let Some(rule) = (unsafe { workspaces::match_rule_for_window(hwnd, rules) }) else {
            return false;
        };
        log_info!(
            "{} window {:?} under rule '{}' -> Display {}, Space {}",
            what,
            hwnd,
            rule.name,
            rule.display_index + 1,
            rule.space_index + 1
        );
        self.place_by_rule(hwnd, &rule);
        self.switch_space(rule.display_index, rule.space_index, Some(hwnd));
        true
    }
}
