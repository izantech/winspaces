//! The four `WM_WINSPACES_*` IPC arms posted by the settings process (a
//! separate exe instance) to reach the running daemon.

use winspaces_common::{log_info, log_warn, Config};
use winspaces_core::hotkeys::HotkeyManager;
use winspaces_core::workspaces;
use winspaces_ui::overview;

use crate::app::{update_state_tray_icon, with_app_state, AppState};
use crate::handlers::winevents::update_foreground_hook;
use crate::restore::restore_workspace_rules;

pub(crate) fn on_reload_config() {
    log_info!("Received configuration reload IPC message from GUI");
    with_app_state(|state| {
        let new_config = Config::load_from_file(&Config::get_config_path());
        apply_config(state, new_config);
    });
}

/// The one place a freshly loaded config is pushed into daemon state. Both
/// the IPC reload and the tray "Reload Configuration" come through here so
/// the two paths cannot drift (the tray path used to skip gaps, the tiling
/// enable flag and float rules).
pub(crate) fn apply_config(state: &mut AppState, new_config: Config) {
    // First: the tray menu, Overview and the indicator read the
    // language when they build or paint, so nothing else needs a nudge.
    let lang = winspaces_common::Lang::resolve(&new_config.language);
    winspaces_common::i18n::set_current(lang);
    log_info!(
        "UI language: {:?} (setting {:?})",
        lang,
        new_config.language
    );
    state.config = new_config.clone();
    state
        .space_mgr
        .set_show_all_taskbar(new_config.show_all_taskbar);
    state.space_mgr.space_indicator = new_config.space_indicator;
    let new_gaps = winspaces_core::tiling::Gaps {
        inner: new_config.tiling.inner_gap,
        outer: new_config.tiling.outer_gap,
    };
    if state.space_mgr.tiling_gaps != new_gaps {
        state.space_mgr.tiling_gaps = new_gaps;
        if state.space_mgr.tiling_enabled {
            state.space_mgr.mark_all_tiling_dirty();
        }
    }
    state
        .space_mgr
        .set_tiling_enabled(new_config.tiling.enabled);
    state.space_mgr.float_rules = new_config.tiling.float_rules.clone();
    update_foreground_hook(state);

    HotkeyManager::unregister_all();
    if !HotkeyManager::register_all(&state.config, state.space_mgr.max_space_count()) {
        log_warn!("Hotkey registration failed after config reload.");
    }
    update_state_tray_icon(state);
}

pub(crate) fn on_capture_workspace() {
    log_info!("Received capture workspace IPC message from GUI");
    with_app_state(|state| {
        let rules = unsafe { workspaces::capture_active_workspace(&state.space_mgr) };
        log_info!("Captured {} workspace rules from layout", rules.len());
        state.config.workspace_rules = rules;
        crate::app::persist_config(&state.config);
    });
}

pub(crate) fn on_restore_workspace() {
    log_info!("Received restore workspace IPC message from GUI");
    with_app_state(|state| {
        restore_workspace_rules(state);
    });
}

pub(crate) fn on_toggle_overview() {
    log_info!("Received toggle overview IPC message");
    with_app_state(|state| {
        overview::toggle_overview(&mut state.space_mgr);
    });
}

pub(crate) fn on_tiling_toggle() {
    log_info!("Received toggle tiling IPC message");
    with_app_state(|state| {
        crate::spaces::toggle_tiling(state);
    });
}
