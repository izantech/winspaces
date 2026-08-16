//! The message-window procedure: pure dispatch. Each arm forwards to its
//! handler module; no handling logic lives here.

use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    WM_ACTIVATE, WM_COMMAND, WM_DESTROY, WM_DISPLAYCHANGE, WM_ENDSESSION, WM_TIMER,
    WM_WTSSESSION_CHANGE,
};
use winspaces_common::{
    WM_WINSPACES_CAPTURE_WORKSPACE, WM_WINSPACES_RELOAD_CONFIG, WM_WINSPACES_RESTORE_WORKSPACE,
    WM_WINSPACES_RETILE, WM_WINSPACES_TILING_TOGGLE, WM_WINSPACES_TOGGLE_MISSION_CONTROL,
};
use winspaces_ui::tray::WM_TRAYICON;

use crate::handlers::{commands, ipc, session, shell};

pub(crate) extern "system" fn wndproc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    unsafe {
        match msg {
            WM_TRAYICON => {
                commands::on_tray_icon(hwnd, lparam);
                0
            }
            WM_COMMAND => {
                commands::on_command(wparam);
                0
            }
            WM_WINSPACES_RELOAD_CONFIG => {
                ipc::on_reload_config();
                0
            }
            WM_WINSPACES_CAPTURE_WORKSPACE => {
                ipc::on_capture_workspace();
                0
            }
            WM_WINSPACES_RESTORE_WORKSPACE => {
                ipc::on_restore_workspace();
                0
            }
            WM_WINSPACES_TOGGLE_MISSION_CONTROL => {
                ipc::on_toggle_mission_control();
                0
            }
            WM_WINSPACES_TILING_TOGGLE => {
                ipc::on_tiling_toggle();
                0
            }
            WM_WINSPACES_RETILE => {
                session::on_retile_request(hwnd);
                0
            }

            WM_ACTIVATE => {
                session::on_activate(wparam);
                0
            }
            WM_DISPLAYCHANGE => {
                session::on_display_change(hwnd);
                0
            }
            WM_WTSSESSION_CHANGE => {
                session::on_wtssession_change(hwnd, wparam);
                0
            }
            WM_TIMER => {
                session::on_timer(hwnd, wparam);
                0
            }
            WM_ENDSESSION => {
                session::on_end_session(wparam);
                0
            }
            WM_DESTROY => {
                session::on_destroy();
                0
            }
            _ => shell::on_shell_hook(hwnd, msg, wparam, lparam),
        }
    }
}
