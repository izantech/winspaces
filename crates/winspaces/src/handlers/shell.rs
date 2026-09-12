//! The ShellHook messages: tracking newly created windows on the space the
//! user is on, untracking destroyed ones, and the activation path that
//! follows the user to a window's space (also reached from the foreground
//! WinEvent hook).

use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    DefWindowProcW, SetTimer, HSHELL_WINDOWACTIVATED, HSHELL_WINDOWCREATED, HSHELL_WINDOWDESTROYED,
};
use winspaces_common::log_info;
use winspaces_core::spaces;
use winspaces_core::spaces::ActivationDecision;
use winspaces_ui::overview;

use crate::app::{with_app_state, AppState, APP_STATE};
use crate::handlers::session::{CLOSE_VERIFY_MS, TIMER_CLOSE_VERIFY};

const HSHELL_RUDEAPPACTIVATED: u32 = HSHELL_WINDOWACTIVATED | 0x8000;

/// The ShellHook message id is registered dynamically and lives on
/// `AppState`; anything that isn't it falls back to `DefWindowProcW`, same as
/// the pre-split default arm.
pub(crate) unsafe fn on_shell_hook(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    let shell_hook_id = APP_STATE.with(|s| {
        s.try_borrow()
            .ok()
            .and_then(|st| st.as_ref().map(|s| s.shell_hook_msg))
            .unwrap_or(0)
    });
    if shell_hook_id != 0 && msg == shell_hook_id {
        let event = wparam as u32;
        let target_hwnd = lparam as HWND;
        if event == HSHELL_WINDOWCREATED {
            // A new window lands on the space the user is looking at. The
            // workspace rules are deliberately not consulted here: they
            // describe a captured layout and are replayed only by the
            // startup and manual restores, never onto windows opened later.
            with_app_state(|state| {
                state.space_mgr.scan_untracked_windows();
            });
        } else if event == HSHELL_WINDOWDESTROYED {
            // Without this, a closed window's handle stays in the tracked list
            // for the daemon's lifetime: `remove_window` is otherwise only
            // reached when a window *moves* between spaces, and `EnumWindows`
            // can't notice the gap because it only yields live windows.
            // `scan_untracked_windows` prunes too, but only when something
            // triggers a scan; this untracks at the moment of closing.
            //
            // Guarded on liveness, and that guard is load-bearing: the shell
            // sends this event when a window leaves its window list, which our
            // own cloaking can cause on a space switch. Untracking then would
            // silently discard the user's space assignment for a window that
            // is merely hidden. Only a genuinely dead handle gets dropped.
            with_app_state(|state| {
                if spaces::is_live_window(target_hwnd) {
                    // Window is still live at the moment the event arrived (common
                    // race during app close, or a cloak-induced destroy event).
                    // Arm the one-shot verify timer so dead handles are pruned
                    // and surviving tiled windows re-expand once teardown completes.
                    unsafe {
                        SetTimer(
                            state.message_hwnd,
                            TIMER_CLOSE_VERIFY,
                            CLOSE_VERIFY_MS,
                            None,
                        );
                    }
                    return;
                }
                if state.space_mgr.remove_window(target_hwnd) {
                    log_info!("ShellHook: untracked destroyed window {:?}", target_hwnd);
                    // An open overlay is showing a card for a window that no
                    // longer exists; re-sync it in place.
                    if overview::is_overview_active() {
                        overview::refresh_overview(&mut state.space_mgr);
                    }
                }
            });
        } else if event == HSHELL_WINDOWACTIVATED
            || event == HSHELL_RUDEAPPACTIVATED
            || (event & 0x7FFF) == HSHELL_WINDOWACTIVATED
        {
            with_app_state(|state| {
                handle_window_activated(target_hwnd, state);
            });
        }
        0
    } else {
        DefWindowProcW(hwnd, msg, wparam, lparam)
    }
}

/// Follow the user to the activated window's space. The decision lives in
/// `SpaceManager::resolve_activation`; this runs inside `with_app_state`,
/// like every other daemon-state mutation, and only performs the switch.
pub(crate) fn handle_window_activated(hwnd: HWND, state: &mut AppState) {
    match state.space_mgr.resolve_activation(hwnd) {
        ActivationDecision::Ignore => {}
        ActivationDecision::Switch {
            hwnd,
            mon_idx,
            space_idx,
        } => {
            state.space_mgr.suppress_foreground = true;
            state.space_mgr.switch_space(mon_idx, space_idx, Some(hwnd));
            state.space_mgr.suppress_foreground = false;
            crate::app::update_state_tray_icon(state);
        }
    }
}
