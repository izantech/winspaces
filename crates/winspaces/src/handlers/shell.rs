//! Foreground-window tracking and the low-level input hooks behind it: the
//! keyboard hook's Win+Tab interception, the ShellHook default case that
//! auto-places newly created windows, and the WinEvent hook that follows
//! foreground changes across spaces.

use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    DefWindowProcW, GetAncestor, GetClassNameW, GetWindowTextW, SetTimer, GA_ROOTOWNER,
    HSHELL_WINDOWACTIVATED, HSHELL_WINDOWCREATED, HSHELL_WINDOWDESTROYED, KBDLLHOOKSTRUCT,
    WM_KEYDOWN, WM_SYSKEYDOWN,
};
use winspaces_common::{log_info, WM_WINSPACES_TOGGLE_MISSION_CONTROL};
use winspaces_core::{spaces, workspaces};
use winspaces_ui::{menu, mission_control};
use winspaces_win32::hooks::WinEventHook;

use crate::app::{with_app_state, AppState, APP_STATE};
use crate::handlers::session::{CLOSE_VERIFY_MS, TIMER_CLOSE_VERIFY};

const HSHELL_RUDEAPPACTIVATED: u32 = HSHELL_WINDOWACTIVATED | 0x8000;

pub(crate) unsafe extern "system" fn low_level_keyboard_proc(
    code: i32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    if code >= 0 && (wparam == WM_KEYDOWN as usize || wparam == WM_SYSKEYDOWN as usize) {
        let kbd = &*(lparam as *const KBDLLHOOKSTRUCT);
        // While the custom tray menu is open it owns the keyboard: navigation
        // keys are re-posted to the menu window and swallowed here (the menu
        // never activates, so no window has focus to receive them natively).
        if menu::is_menu_open() && menu::forward_key(kbd.vkCode) {
            return 1;
        }
        if kbd.vkCode == windows_sys::Win32::UI::Input::KeyboardAndMouse::VK_TAB as u32 {
            let win_down = (windows_sys::Win32::UI::Input::KeyboardAndMouse::GetAsyncKeyState(
                windows_sys::Win32::UI::Input::KeyboardAndMouse::VK_LWIN as i32,
            ) as u16
                & 0x8000
                != 0)
                || (windows_sys::Win32::UI::Input::KeyboardAndMouse::GetAsyncKeyState(
                    windows_sys::Win32::UI::Input::KeyboardAndMouse::VK_RWIN as i32,
                ) as u16
                    & 0x8000
                    != 0);
            if win_down {
                // Do the minimum inside the LL hook: exceeding the system's
                // low-level-hook timeout gets the hook silently uninstalled.
                // Post the toggle to the message loop instead.
                let mut handled = false;
                APP_STATE.with(|s| {
                    if let Ok(state_opt) = s.try_borrow() {
                        if let Some(state) = state_opt.as_ref() {
                            if state.config.intercept_win_tab {
                                windows_sys::Win32::UI::WindowsAndMessaging::PostMessageW(
                                    state.message_hwnd,
                                    WM_WINSPACES_TOGGLE_MISSION_CONTROL,
                                    0,
                                    0,
                                );
                                handled = true;
                            }
                        }
                    }
                });
                if handled {
                    // Inject a no-op key so the swallowed Tab still counts as
                    // "a key was pressed while Win was down" — otherwise the
                    // Start menu opens when the Win key is released.
                    windows_sys::Win32::UI::Input::KeyboardAndMouse::keybd_event(0xFF, 0, 0, 0);
                    windows_sys::Win32::UI::Input::KeyboardAndMouse::keybd_event(
                        0xFF,
                        0,
                        windows_sys::Win32::UI::Input::KeyboardAndMouse::KEYEVENTF_KEYUP,
                        0,
                    );
                    return 1;
                }
            }
        }
    }
    windows_sys::Win32::UI::WindowsAndMessaging::CallNextHookEx(
        std::ptr::null_mut(),
        code,
        wparam,
        lparam,
    )
}

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
                if state.config.auto_restore_workspaces {
                    if let Some(rule) = workspaces::match_rule_for_window(
                        target_hwnd,
                        &state.config.workspace_rules,
                    ) {
                        log_info!(
                            "ShellHook auto-placing window {:?} under rule '{}' -> Display {}, Space {}",
                            target_hwnd,
                            rule.name,
                            rule.display_index + 1,
                            rule.space_index + 1
                        );
                        let target = state
                            .space_mgr
                            .monitors
                            .get(rule.display_index)
                            .map(|m| m.hmon);
                        workspaces::apply_rule_to_window(target_hwnd, &rule, target);
                        state.space_mgr.track_window(
                            target_hwnd,
                            rule.display_index,
                            rule.space_index,
                        );
                        if rule.is_sticky {
                            state.space_mgr.set_sticky(target_hwnd, true);
                        }
                        state.space_mgr.switch_space(
                            rule.display_index,
                            rule.space_index,
                            Some(target_hwnd),
                        );
                        return;
                    }
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
    let (target_hwnd, (mon_idx, space_idx)) = if !root.is_null() && root != hwnd {
        match state.space_mgr.find_window(root) {
            Some(loc) => (root, loc),
            None => match state.space_mgr.find_window(hwnd) {
                Some(loc) => (hwnd, loc),
                None => return,
            },
        }
    } else {
        match state.space_mgr.find_window(hwnd) {
            Some(loc) => (hwnd, loc),
            None => return,
        }
    };

    // 3. Check suppression timer on that monitor
    let now = unsafe { windows_sys::Win32::System::SystemInformation::GetTickCount() };
    let mon = &mut state.space_mgr.monitors[mon_idx];
    if mon.suppress_foreground_until != 0 {
        if (now as i32).wrapping_sub(mon.suppress_foreground_until as i32) < 0 {
            return;
        }
        mon.suppress_foreground_until = 0;
    }

    // 4. If window is already on the active space of that monitor, nothing to switch
    if mon.current == space_idx {
        return;
    }

    // 5. A switch is about to happen: now the eligibility probe is worth its
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

    // 6. Perform the space switch on that monitor and update tray icon
    state.space_mgr.suppress_foreground = true;
    state
        .space_mgr
        .switch_space(mon_idx, space_idx, Some(target_hwnd));
    state.space_mgr.suppress_foreground = false;
    crate::app::update_state_tray_icon(state);
}

pub(crate) unsafe extern "system" fn foreground_hook_proc(
    _: windows_sys::Win32::UI::Accessibility::HWINEVENTHOOK,
    _: u32,
    hwnd: HWND,
    id_object: i32,
    id_child: i32,
    _: u32,
    _: u32,
) {
    if id_object != 0 || id_child != 0 || hwnd.is_null() {
        return;
    }

    // This runs inside a WinEvent callback on every foreground change; the
    // class is compared against the raw UTF-16 buffer, and the title — only
    // consulted for CoreWindow hosts — is fetched just for that class. The
    // common path allocates nothing.
    fn utf16_eq(units: &[u16], ascii: &str) -> bool {
        units.len() == ascii.len() && units.iter().zip(ascii.bytes()).all(|(&u, b)| u == b as u16)
    }

    let mut class_buf = [0u16; 256];
    let len = GetClassNameW(hwnd, class_buf.as_mut_ptr(), 256);
    let class: &[u16] = if len > 0 {
        &class_buf[..len as usize]
    } else {
        &[]
    };

    let mut title = String::new();
    let is_task_view = utf16_eq(class, "MultitaskingViewHost")
        || utf16_eq(class, "XamlExplorerHost")
        || (utf16_eq(class, "Windows.UI.Core.CoreWindow") && {
            let mut title_buf = [0u16; 256];
            let tlen = GetWindowTextW(hwnd, title_buf.as_mut_ptr(), 256);
            if tlen > 0 {
                title = String::from_utf16_lossy(&title_buf[..tlen as usize]);
            }
            title == "Task View"
                || title == "Vista de tareas"
                || title == "MultitaskingView"
                || title.contains("Task View")
        });

    if is_task_view {
        log_info!(
            "Intercepted native Windows Task View window (hwnd: {:?}, class: '{}', title: '{}')",
            hwnd,
            String::from_utf16_lossy(class),
            title
        );
        with_app_state(|state| {
            if state.config.intercept_win_tab {
                // Dismiss native Task View
                windows_sys::Win32::UI::Input::KeyboardAndMouse::keybd_event(
                    windows_sys::Win32::UI::Input::KeyboardAndMouse::VK_ESCAPE as u8,
                    0,
                    0,
                    0,
                );
                windows_sys::Win32::UI::Input::KeyboardAndMouse::keybd_event(
                    windows_sys::Win32::UI::Input::KeyboardAndMouse::VK_ESCAPE as u8,
                    0,
                    windows_sys::Win32::UI::Input::KeyboardAndMouse::KEYEVENTF_KEYUP,
                    0,
                );
                mission_control::show_mission_control(&mut state.space_mgr);
            }
        });
        return;
    }

    with_app_state(|state| {
        handle_window_activated(hwnd, state);
    });
}

pub(crate) fn update_foreground_hook(state: &mut AppState) {
    if state._win_event_hook.is_none() {
        state._win_event_hook = WinEventHook::install(foreground_hook_proc);
    }
}

pub(crate) unsafe extern "system" fn minimize_hook_proc(
    _: windows_sys::Win32::UI::Accessibility::HWINEVENTHOOK,
    _: u32,
    hwnd: HWND,
    id_object: i32,
    id_child: i32,
    _: u32,
    _: u32,
) {
    if id_object != 0 || id_child != 0 || hwnd.is_null() {
        return;
    }
    with_app_state(|state| {
        if state.space_mgr.tiling_enabled {
            if let Some((m_idx, s_idx)) = state.space_mgr.find_window(hwnd) {
                state.space_mgr.mark_tiling_dirty(m_idx, s_idx);
            }
        }
    });
}

pub(crate) unsafe extern "system" fn movesize_hook_proc(
    _: windows_sys::Win32::UI::Accessibility::HWINEVENTHOOK,
    event: u32,
    hwnd: HWND,
    id_object: i32,
    id_child: i32,
    _: u32,
    _: u32,
) {
    if id_object != 0 || id_child != 0 || hwnd.is_null() {
        return;
    }
    with_app_state(|state| {
        if event == winspaces_win32::hooks::EVENT_SYSTEM_MOVESIZESTART {
            state.space_mgr.tiling_on_movesize_start(hwnd);
        } else if event == winspaces_win32::hooks::EVENT_SYSTEM_MOVESIZEEND {
            state.space_mgr.tiling_on_movesize_end(hwnd);
        }
    });
}
