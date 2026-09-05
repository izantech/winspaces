//! Workspace-rule window placement: apply saved workspace rules to already
//! open windows, either at startup or on demand from the tray/IPC.

use windows_sys::Win32::Foundation::HWND;
use windows_sys::Win32::UI::WindowsAndMessaging::EnumWindows;
use winspaces_common::log_info;
use winspaces_core::{layout_store, workspaces};

use crate::app::AppState;

pub(crate) fn restore_workspace_rules(state: &mut AppState) {
    if state.config.workspace_rules.is_empty() {
        return;
    }
    unsafe {
        EnumWindows(Some(restore_enum_proc), state as *mut _ as isize);
    }
    for mon_idx in 0..state.space_mgr.monitors.len() {
        let cur = state.space_mgr.monitors[mon_idx].current;
        state.space_mgr.switch_space(mon_idx, cur, None);
    }
    state.space_mgr.begin_settle(layout_store::SETTLE_MS);
}

unsafe extern "system" fn restore_enum_proc(hwnd: HWND, lparam: isize) -> i32 {
    let state = &mut *(lparam as *mut AppState);
    if let Some(rule) = workspaces::match_rule_for_window(hwnd, &state.config.workspace_rules) {
        log_info!(
            "Restoring window {:?} under rule '{}' -> Display {}, Space {}",
            hwnd,
            rule.name,
            rule.display_index + 1,
            rule.space_index + 1
        );
        state.space_mgr.place_by_rule(hwnd, &rule);
    }
    1
}
