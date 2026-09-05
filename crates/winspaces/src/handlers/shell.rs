//! Foreground-window tracking and the low-level input hooks behind it: the
//! keyboard hook's Win+Tab interception, the ShellHook default case that
//! auto-places newly created windows, and the WinEvent hook that follows
//! foreground changes across spaces.

use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    DefWindowProcW, GetAncestor, SetTimer, GA_ROOTOWNER, HSHELL_WINDOWACTIVATED,
    HSHELL_WINDOWCREATED, HSHELL_WINDOWDESTROYED,
};
use winspaces_common::log_info;
use winspaces_core::spaces;
use winspaces_core::spaces::RehomeTrigger;
use winspaces_ui::mission_control;

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
            with_app_state(|state| {
                if state.config.auto_restore_workspaces
                    && state.space_mgr.try_place_by_rule(
                        target_hwnd,
                        &state.config.workspace_rules,
                        "ShellHook auto-placing",
                    )
                {
                    crate::app::update_state_tray_icon(state);
                    return;
                }
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
                    if mission_control::is_mission_control_active() {
                        mission_control::refresh_mission_control(&mut state.space_mgr);
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

pub(crate) fn handle_window_activated(hwnd: HWND, state: &mut AppState) {
    // A pinned window is on screen on every space, so activating one says
    // nothing about where the user wants to be. This guard is load-bearing,
    // not defensive: `find_window` reports a window's real home space, so
    // without it, clicking a window pinned from Space 1 while standing on
    // Space 3 would drag the user back to Space 1.
    if hwnd.is_null() || state.space_mgr.suppress_foreground || state.space_mgr.is_sticky(hwnd) {
        return;
    }

    // Most activations are for a window already on its monitor's active space
    // and end in a no-op — and this runs twice per activation (WinEvent +
    // ShellHook, deliberately dual). So everything up to the switch decision
    // is answered from the tracked set in memory; the eligibility probe, with
    // its cross-process DWM cloak query, is deferred to the switch path.

    // 1. Resolve to root owner window if needed (e.g. child, dialog, or owned popup)
    let root = unsafe { GetAncestor(hwnd, GA_ROOTOWNER) };
    if !root.is_null() && state.space_mgr.is_sticky(root) {
        return;
    }

    // 2. Find the tracked location: the root's, or the activated hwnd's
    let target_hwnd =
        if !root.is_null() && root != hwnd && state.space_mgr.find_window(root).is_some() {
            root
        } else {
            hwnd
        };

    let (mon_idx, space_idx) = match state.space_mgr.find_window(target_hwnd) {
        Some(loc) => loc,
        None => {
            // Target window is not yet tracked; if valid, track it immediately
            if !spaces::is_valid_window(target_hwnd) {
                return;
            }
            if state.config.auto_restore_workspaces
                && state.space_mgr.try_place_by_rule(
                    target_hwnd,
                    &state.config.workspace_rules,
                    "Auto-placing newly activated",
                )
            {
                crate::app::update_state_tray_icon(state);
                return;
            }
            match state.space_mgr.adopt_at_current(target_hwnd) {
                Some((actual_mon, cur_space)) => {
                    log_info!(
                        "Newly activated window {:?} -> tracked to Mon {}, Space {}",
                        target_hwnd,
                        actual_mon + 1,
                        cur_space + 1
                    );
                    (actual_mon, cur_space)
                }
                None => return,
            }
        }
    };

    // 3. Check if the window moved across displays through a move that fired no
    //    MOVESIZE or foreground event (a programmatic SetWindowPos, a monitor
    //    reflow); the user's own drags are handled by the movesize hook.
    if let Some(actual_mon) = state.space_mgr.monitor_index_for_hwnd(target_hwnd) {
        if actual_mon != mon_idx
            && state.space_mgr.adopt_cross_monitor_move(
                target_hwnd,
                mon_idx,
                actual_mon,
                RehomeTrigger::Activation,
            )
        {
            return;
        }
    }

    // 4. Check suppression timer on that monitor
    let now = unsafe { windows_sys::Win32::System::SystemInformation::GetTickCount() };
    let mon = &mut state.space_mgr.monitors[mon_idx];
    if mon.suppress_foreground_until != 0 {
        if (now as i32).wrapping_sub(mon.suppress_foreground_until as i32) < 0 {
            return;
        }
        mon.suppress_foreground_until = 0;
    }

    // 5. If window is already on the active space of that monitor, nothing to switch
    if mon.current == space_idx {
        return;
    }

    // 6. A switch is about to happen: now the eligibility probe is worth its
    // cost. A tracked but externally-cloaked window must not trigger one.
    if !spaces::is_valid_window(target_hwnd) {
        return;
    }

    log_info!(
        "Window activation for {:?} -> Switching Display {} from Space {} to Space {}",
        target_hwnd,
        mon_idx + 1,
        mon.current + 1,
        space_idx + 1
    );

    // 7. Perform the space switch on that monitor and update tray icon
    state.space_mgr.suppress_foreground = true;
    state
        .space_mgr
        .switch_space(mon_idx, space_idx, Some(target_hwnd));
    state.space_mgr.suppress_foreground = false;
    crate::app::update_state_tray_icon(state);
}
