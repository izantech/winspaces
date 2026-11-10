//! Remote Desktop pause: with `pause_in_remote_session` on, the daemon stands
//! down while the session is driven over RDP and comes back through the
//! normal topology reconcile, which restores the desk layout.

use windows_sys::Win32::UI::WindowsAndMessaging::{
    DeregisterShellHookWindow, RegisterShellHookWindow,
};
use winspaces_common::{log_info, log_warn, Config};
use winspaces_core::hotkeys::HotkeyManager;
use winspaces_core::topology;
use winspaces_ui::overview;

use crate::app::{AppState, Hooks};
use crate::shadow::persist_shadow;

pub(crate) fn wanted(config: &Config) -> bool {
    config.pause_in_remote_session && topology::is_remote_session()
}

pub(crate) fn enter(state: &mut AppState) {
    if state.paused {
        return;
    }
    log_info!("Remote session: pausing WinSpaces");
    state.paused = true;
    // The pending shadow is the last desk capture; keep it.
    persist_shadow(state);
    state.shadow = None;
    // Whatever topology comes back next must replay its stored layout.
    state.last_signature.clear();

    overview::hide_overview();
    HotkeyManager::unregister_all();
    state.hooks = None;
    unsafe {
        DeregisterShellHookWindow(state.message_hwnd);
    }
    state.space_mgr.set_tiling_enabled(false);
    state.space_mgr.windows_show_all();
    // Rebuilt from scratch by the reconcile that ends the pause.
    state.space_mgr.monitors.clear();
    crate::spaces::refresh_shelf(state);
}

/// Reinstall what `enter` took down. Returns whether a pause ended; the
/// caller re-registers hotkeys once the monitors and space counts are back.
pub(crate) fn leave(state: &mut AppState) -> bool {
    if !state.paused {
        return false;
    }
    log_info!("Leaving remote-session pause");
    state.paused = false;
    state.hooks = Some(Hooks::install());
    unsafe {
        RegisterShellHookWindow(state.message_hwnd);
    }
    state
        .space_mgr
        .set_tiling_enabled(state.config.tiling.enabled);
    true
}

pub(crate) fn register_hotkeys(state: &mut AppState) {
    if state.space_mgr.handle_hotkeys
        && !HotkeyManager::register_all(&state.config, state.space_mgr.max_space_count())
    {
        log_warn!("Hotkey registration failed after the remote-session pause");
    }
}

/// Tray commands that still make sense while paused; everything else
/// acts on spaces that are not there.
pub(crate) fn allows_command(cmd: usize) -> bool {
    use crate::handlers::commands::{
        ID_TRAY_CHECK_UPDATES, ID_TRAY_CONFIG, ID_TRAY_EXIT, ID_TRAY_RELOAD,
    };
    matches!(
        cmd,
        ID_TRAY_CONFIG | ID_TRAY_CHECK_UPDATES | ID_TRAY_RELOAD | ID_TRAY_EXIT
    )
}
