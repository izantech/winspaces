//! Workspace-rule placement: the one path the startup restore and the
//! tray/IPC "Restore Workspace" use to put an already open window where a
//! `WorkspaceRule` says it belongs.
//!
//! Rules describe a captured layout, not a standing order: they identify an
//! app, so applying them to windows created later would send every new
//! window of a captured app to the captured space and drag the user along.
//! A window that appears after startup is adopted on the space the user is
//! on (`rehome::adopt_at_current`), whatever the rules say.

use windows_sys::Win32::Foundation::HWND;
use winspaces_common::WorkspaceRule;

use super::manager::SpaceManager;
use crate::workspaces;

impl SpaceManager {
    /// Apply `rule` to `hwnd`: move it onto the rule's display and saved
    /// geometry, track it on the rule's space, and pin it when the rule says
    /// so. Does not switch spaces; the restore paths refresh every monitor's
    /// current space once all windows are placed.
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
}
