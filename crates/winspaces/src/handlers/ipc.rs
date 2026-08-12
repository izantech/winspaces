//! The four `WM_WINSPACES_*` IPC arms posted by the settings process (a
//! separate exe instance) to reach the running daemon.

use winspaces_common::{log_info, log_warn, Config};
use winspaces_core::hotkeys::HotkeyManager;
use winspaces_core::workspaces;
use winspaces_ui::mission_control;

use crate::app::{update_state_tray_icon, with_app_state};
use crate::handlers::shell::update_foreground_hook;
use crate::restore::restore_workspace_rules;

pub(crate) fn on_reload_config() {
    log_info!("Received configuration reload IPC message from GUI");
    with_app_state(|state| {
        let path = Config::get_config_path();
        let new_config = Config::load_from_file(&path);
        state.config = new_config.clone();
        state
            .desktop_mgr
            .set_show_all_taskbar(new_config.show_all_taskbar);
        state.desktop_mgr.space_indicator = new_config.space_indicator;
        update_foreground_hook(state);
        HotkeyManager::unregister_all();
        if !HotkeyManager::register_all(&state.config, state.desktop_mgr.max_space_count()) {
            log_warn!("Hotkey registration failed after IPC config reload.");
        }
        update_state_tray_icon(state);
    });
}

pub(crate) fn on_capture_workspace() {
    log_info!("Received capture workspace IPC message from GUI");
    with_app_state(|state| {
        let rules = unsafe { workspaces::capture_active_workspace(&state.desktop_mgr) };
        log_info!("Captured {} workspace rules from layout", rules.len());
        state.config.workspace_rules = rules;
        let path = Config::get_config_path();
        let _ = state.config.save_to_file(&path);
    });
}

pub(crate) fn on_restore_workspace() {
    log_info!("Received restore workspace IPC message from GUI");
    with_app_state(|state| {
        restore_workspace_rules(state);
    });
}

pub(crate) fn on_toggle_mission_control() {
    log_info!("Received toggle mission control IPC message");
    with_app_state(|state| {
        mission_control::toggle_mission_control(&mut state.desktop_mgr);
    });
}
