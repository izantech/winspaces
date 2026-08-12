//! Foreground-window tracking and the low-level input hooks behind it: the
//! keyboard hook's Win+Tab interception, the ShellHook default case that
//! auto-places newly created windows, and the WinEvent hook that follows
//! foreground changes across spaces.

use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    DefWindowProcW, GetAncestor, GetClassNameW, GetWindowTextW, GA_ROOTOWNER,
    HSHELL_WINDOWACTIVATED, HSHELL_WINDOWCREATED, HSHELL_WINDOWDESTROYED, KBDLLHOOKSTRUCT,
    WM_KEYDOWN, WM_SYSKEYDOWN,
};
use winspaces_common::{log_info, WM_WINSPACES_TOGGLE_MISSION_CONTROL};
use winspaces_core::{desktop, workspaces};
use winspaces_ui::{menu, mission_control};
use winspaces_win32::hooks::WinEventHook;

use crate::app::{with_app_state, AppState, APP_STATE};

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
                            rule.desktop_index + 1
                        );
                        let target = state
                            .desktop_mgr
                            .monitors
                            .get(rule.display_index)
                            .map(|m| m.hmon);
                        workspaces::apply_rule_to_window(target_hwnd, &rule, target);
                        state.desktop_mgr.track_window(
                            target_hwnd,
                            rule.display_index,
                            rule.desktop_index,
                        );
                        state.desktop_mgr.switch_desktop(
                            rule.display_index,
                            rule.desktop_index,
                            Some(target_hwnd),
                        );
                        return;
                    }
                }
                state.desktop_mgr.scan_untracked_windows();
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
                if desktop::is_live_window(target_hwnd) {
                    return;
                }
                if state.desktop_mgr.remove_window(target_hwnd) {
                    log_info!("ShellHook: untracked destroyed window {:?}", target_hwnd);
                    // An open overlay is showing a card for a window that no
                    // longer exists; re-sync it in place.
                    if mission_control::is_mission_control_active() {
                        mission_control::refresh_mission_control(&mut state.desktop_mgr);
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
    if hwnd.is_null() || state.desktop_mgr.suppress_foreground {
        return;
    }

    // 1. Resolve to root owner window if needed (e.g. child, dialog, or owned popup)
    let target_hwnd = unsafe {
        let root = GetAncestor(hwnd, GA_ROOTOWNER);
        if !root.is_null() && desktop::is_valid_window(root) {
            root
        } else {
            hwnd
        }
    };

    if !desktop::is_valid_window(target_hwnd) {
        return;
    }

    // 2. Find tracked location of target window (or original hwnd as fallback)
    let (mon_idx, desk_idx) = match state.desktop_mgr.find_window(target_hwnd) {
        Some(loc) => loc,
        None => match state.desktop_mgr.find_window(hwnd) {
            Some(loc) => loc,
            None => return,
        },
    };

    // 3. Check suppression timer on that monitor
    let now = unsafe { windows_sys::Win32::System::SystemInformation::GetTickCount() };
    let mon = &mut state.desktop_mgr.monitors[mon_idx];
    if mon.suppress_foreground_until != 0 {
        if (now as i32).wrapping_sub(mon.suppress_foreground_until as i32) < 0 {
            return;
        }
        mon.suppress_foreground_until = 0;
    }

    // 4. If window is already on the active space of that monitor, nothing to switch
    if mon.current == desk_idx {
        return;
    }

    log_info!(
        "Window activation for {:?} -> Switching Display {} from Space {} to Space {}",
        target_hwnd,
        mon_idx + 1,
        mon.current + 1,
        desk_idx + 1
    );

    // 5. Perform the desktop switch on that monitor and update tray icon
    state.desktop_mgr.suppress_foreground = true;
    state
        .desktop_mgr
        .switch_desktop(mon_idx, desk_idx, Some(target_hwnd));
    state.desktop_mgr.suppress_foreground = false;
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

    let mut class_buf = [0u16; 256];
    let len = GetClassNameW(hwnd, class_buf.as_mut_ptr(), 256);
    let class_name = if len > 0 {
        String::from_utf16_lossy(&class_buf[..len as usize])
    } else {
        String::new()
    };

    let mut title_buf = [0u16; 256];
    let tlen = GetWindowTextW(hwnd, title_buf.as_mut_ptr(), 256);
    let title = if tlen > 0 {
        String::from_utf16_lossy(&title_buf[..tlen as usize])
    } else {
        String::new()
    };

    let is_task_view = class_name == "MultitaskingViewHost"
        || class_name == "XamlExplorerHost"
        || (class_name == "Windows.UI.Core.CoreWindow"
            && (title == "Task View"
                || title == "Vista de tareas"
                || title == "MultitaskingView"
                || title.contains("Task View")));

    if is_task_view {
        log_info!(
            "Intercepted native Windows Task View window (hwnd: {:?}, class: '{}', title: '{}')",
            hwnd,
            class_name,
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
                mission_control::show_mission_control(&mut state.desktop_mgr);
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
