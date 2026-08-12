//! Tray context-menu command IDs, `WM_TRAYICON`, and the `WM_COMMAND`
//! dispatch for the 11 tray-menu actions.

use std::ptr::null_mut;
use windows_sys::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    PostQuitMessage, SW_SHOWNORMAL, WM_CONTEXTMENU, WM_LBUTTONUP, WM_RBUTTONUP,
};
use winspaces_common::{log_info, Config};
use winspaces_core::hotkeys::HotkeyManager;
use winspaces_core::workspaces;
use winspaces_ui::mission_control;
use winspaces_win32::text::encode_wide;

use crate::app::{launch_settings, update_state_tray_icon, with_app_state};
use crate::handlers::shell::update_foreground_hook;
use crate::restore::restore_workspace_rules;
use crate::spaces::{add_space_on, remove_space_on};
use crate::tray_menu;

// Tray context-menu command IDs.
pub(crate) const ID_TRAY_MISSION_CONTROL: usize = 999;
pub(crate) const ID_TRAY_TOGGLE_TASKBAR: usize = 1000;
pub(crate) const ID_TRAY_CONFIG: usize = 1001;
pub(crate) const ID_TRAY_EXIT: usize = 1002;
pub(crate) const ID_TRAY_RELOAD: usize = 1003;
pub(crate) const ID_TRAY_CAPTURE_WS: usize = 1004;
pub(crate) const ID_TRAY_RESTORE_WS: usize = 1005;
pub(crate) const ID_TRAY_CHECK_UPDATES: usize = 1006;
pub(crate) const ID_TRAY_SWITCH_BASE: usize = 2000;
// Reserved offsets inside each monitor's 100-wide command stride
// (`ID_TRAY_SWITCH_BASE + mon_idx * 100 + offset`). Space indices only ever
// reach MAX_DESKTOPS - 1 = 8, so 98/99 can never collide with a switch.
pub(crate) const TRAY_OFFSET_ADD_SPACE: usize = 98;
pub(crate) const TRAY_OFFSET_REMOVE_SPACE: usize = 99;

/// Manual update affordance: the tray item opens the releases page in the
/// default browser. No network code lives in the daemon.
const UPDATE_URL: &str = "https://github.com/izantech/winspaces/releases/latest";

/// One command hiding behind `cmd >= ID_TRAY_SWITCH_BASE`, decoded from its
/// stride-encoded id. Split out from [`on_command`] so the id arithmetic has
/// exactly one implementation and a build -> id -> decode round trip is
/// something a test can call directly instead of re-deriving.
pub(crate) enum TraySwitchCommand {
    Add { mon_idx: usize },
    Remove { mon_idx: usize },
    Switch { mon_idx: usize, desk_idx: usize },
}

pub(crate) fn decode_switch_command(cmd: usize) -> Option<TraySwitchCommand> {
    if cmd < ID_TRAY_SWITCH_BASE {
        return None;
    }
    let offset = cmd - ID_TRAY_SWITCH_BASE;
    let mon_idx = offset / 100;
    Some(match offset % 100 {
        TRAY_OFFSET_ADD_SPACE => TraySwitchCommand::Add { mon_idx },
        TRAY_OFFSET_REMOVE_SPACE => TraySwitchCommand::Remove { mon_idx },
        desk_idx => TraySwitchCommand::Switch { mon_idx, desk_idx },
    })
}

pub(crate) fn on_tray_icon(hwnd: HWND, lparam: LPARAM) {
    let event = lparam as u32;
    if event == WM_RBUTTONUP || event == WM_CONTEXTMENU {
        log_info!("Tray icon right-clicked");
        tray_menu::show_tray_menu(hwnd);
    } else if event == WM_LBUTTONUP {
        // Every left click toggles instantly. Double-click has no separate
        // meaning (Settings lives in the context menu), so no need to defer
        // past the double-click interval.
        log_info!("Tray icon left-clicked: Toggling Mission Control");
        with_app_state(|state| {
            mission_control::toggle_mission_control(&mut state.desktop_mgr);
        });
    }
}

pub(crate) fn on_command(wparam: WPARAM) {
    let cmd = wparam & 0xffff;
    if cmd == ID_TRAY_MISSION_CONTROL {
        log_info!("Tray menu: Mission Control requested");
        with_app_state(|state| {
            mission_control::toggle_mission_control(&mut state.desktop_mgr);
        });
    } else if cmd == ID_TRAY_TOGGLE_TASKBAR {
        log_info!("Tray menu: Toggle taskbar mode");
        with_app_state(|state| {
            let new_val = !state.config.show_all_taskbar;
            state.desktop_mgr.set_show_all_taskbar(new_val);
            state.config.show_all_taskbar = new_val;
            update_foreground_hook(state);
            let _ = state.config.save_to_file(&Config::get_config_path());
        });
    } else if cmd == ID_TRAY_CONFIG {
        log_info!("Tray menu: Open Settings requested");
        launch_settings();
    } else if cmd == ID_TRAY_CHECK_UPDATES {
        log_info!("Tray menu: Check for updates requested");
        unsafe {
            let verb = encode_wide("open");
            let url = encode_wide(UPDATE_URL);
            windows_sys::Win32::UI::Shell::ShellExecuteW(
                null_mut(),
                verb.as_ptr(),
                url.as_ptr(),
                std::ptr::null(),
                std::ptr::null(),
                SW_SHOWNORMAL,
            );
        }
    } else if cmd == ID_TRAY_RELOAD {
        log_info!("Tray menu: Reload requested");
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
            let _ = HotkeyManager::register_all(&state.config, state.desktop_mgr.max_space_count());
            update_state_tray_icon(state);
        });
    } else if cmd == ID_TRAY_CAPTURE_WS {
        log_info!("Tray menu: Capture Workspace requested");
        with_app_state(|state| {
            let rules = unsafe { workspaces::capture_active_workspace(&state.desktop_mgr) };
            log_info!("Captured {} workspace rules", rules.len());
            state.config.workspace_rules = rules;
            let path = Config::get_config_path();
            let _ = state.config.save_to_file(&path);
        });
    } else if cmd == ID_TRAY_RESTORE_WS {
        log_info!("Tray menu: Restore Workspace requested");
        with_app_state(|state| {
            restore_workspace_rules(state);
        });
    } else if cmd == ID_TRAY_EXIT {
        log_info!("Tray menu: Exit requested");
        unsafe {
            PostQuitMessage(0);
        }
    } else if let Some(command) = decode_switch_command(cmd) {
        match command {
            TraySwitchCommand::Add { mon_idx } => {
                log_info!("Tray menu: New space on Monitor {}", mon_idx + 1);
                with_app_state(|state| add_space_on(state, mon_idx));
            }
            TraySwitchCommand::Remove { mon_idx } => {
                log_info!("Tray menu: Remove last space on Monitor {}", mon_idx + 1);
                with_app_state(|state| {
                    // The tray removes the *last* space; targeted removal is
                    // Mission Control's close button.
                    let count = state
                        .desktop_mgr
                        .monitors
                        .get(mon_idx)
                        .map(|m| m.desktops.len())
                        .unwrap_or(0);
                    if count > 1 {
                        remove_space_on(state, mon_idx, count - 1);
                    }
                });
            }
            TraySwitchCommand::Switch { mon_idx, desk_idx } => {
                log_info!(
                    "Tray menu: Switch Monitor {} to Desktop {}",
                    mon_idx + 1,
                    desk_idx + 1
                );
                with_app_state(|state| {
                    state.desktop_mgr.switch_desktop(mon_idx, desk_idx, None);
                    update_state_tray_icon(state);
                });
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn switch_command_id_stride_across_two_monitors() {
        for mon_idx in 0..2usize {
            for desk_idx in 0..5usize {
                let id = ID_TRAY_SWITCH_BASE + mon_idx * 100 + desk_idx;
                match decode_switch_command(id) {
                    Some(TraySwitchCommand::Switch {
                        mon_idx: got_mon,
                        desk_idx: got_desk,
                    }) => {
                        assert_eq!(got_mon, mon_idx);
                        assert_eq!(got_desk, desk_idx);
                    }
                    _ => panic!("expected a SwitchSpace decode for id {}", id),
                }
            }
        }
    }

    #[test]
    fn add_remove_offset_round_trip() {
        // build -> id (as tray_menu::build_menu_entries encodes it) -> decode
        // (the same decode on_command dispatches through), for more than one
        // monitor so a stride error would surface.
        for mon_idx in 0..3usize {
            let add_id = ID_TRAY_SWITCH_BASE + mon_idx * 100 + TRAY_OFFSET_ADD_SPACE;
            match decode_switch_command(add_id) {
                Some(TraySwitchCommand::Add { mon_idx: got }) => assert_eq!(got, mon_idx),
                _ => panic!("expected AddSpace decode for id {}", add_id),
            }

            let remove_id = ID_TRAY_SWITCH_BASE + mon_idx * 100 + TRAY_OFFSET_REMOVE_SPACE;
            match decode_switch_command(remove_id) {
                Some(TraySwitchCommand::Remove { mon_idx: got }) => assert_eq!(got, mon_idx),
                _ => panic!("expected RemoveSpace decode for id {}", remove_id),
            }
        }
    }

    #[test]
    fn below_switch_base_decodes_to_none() {
        assert!(decode_switch_command(ID_TRAY_SWITCH_BASE - 1).is_none());
        assert!(decode_switch_command(0).is_none());
    }
}
