//! WinEvent hook procedures: foreground, show, minimize, move/size and
//! location-change events. Installed `WINEVENT_OUTOFCONTEXT`, so they arrive
//! through the message queue and never re-enter a live `with_app_state`.

use windows_sys::Win32::Foundation::HWND;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    GetAncestor, GetClassNameW, GetWindowTextW, SetTimer, GA_ROOT,
};
use winspaces_common::log_info;
use winspaces_core::spaces;
use winspaces_core::spaces::RehomeTrigger;
use winspaces_ui::overview;
use winspaces_win32::hooks::WinEventHook;

use super::drag_preview::{shift_down, stop_drag_preview, DRAG_PREVIEW_SHIFT};
use super::shell::handle_window_activated;
use crate::app::{with_app_state, AppState};
use crate::handlers::session::{DRAG_PREVIEW_INTERVAL_MS, TIMER_DRAG_PREVIEW};

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

    /// Whether the window's process image is `explorer.exe`. Only reached
    /// for CoreWindow foregrounds, so the process handle round-trip is rare.
    fn owned_by_explorer(hwnd: HWND) -> bool {
        use windows_sys::Win32::Foundation::CloseHandle;
        use windows_sys::Win32::System::Threading::{
            OpenProcess, QueryFullProcessImageNameW, PROCESS_QUERY_LIMITED_INFORMATION,
        };
        use windows_sys::Win32::UI::WindowsAndMessaging::GetWindowThreadProcessId;
        unsafe {
            let mut pid = 0u32;
            GetWindowThreadProcessId(hwnd, &mut pid);
            if pid == 0 {
                return false;
            }
            let process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
            if process.is_null() {
                return false;
            }
            let mut buf = [0u16; 1024];
            let mut len = buf.len() as u32;
            let ok = QueryFullProcessImageNameW(process, 0, buf.as_mut_ptr(), &mut len) != 0;
            CloseHandle(process);
            if !ok {
                return false;
            }
            let path = String::from_utf16_lossy(&buf[..len as usize]).to_ascii_lowercase();
            path.ends_with("\\explorer.exe")
        }
    }

    let mut class_buf = [0u16; 256];
    let len = GetClassNameW(hwnd, class_buf.as_mut_ptr(), 256);
    let class: &[u16] = if len > 0 {
        &class_buf[..len as usize]
    } else {
        &[]
    };

    // A CoreWindow hosted by explorer.exe itself is Task View: the other
    // shell CoreWindows (Start, Search, notification centre) live in their
    // own *Host.exe processes. Checking the owner instead of the caption
    // keeps this working on any Windows display language; the caption is
    // still fetched, but only for the log line.
    let mut title = String::new();
    let is_task_view = utf16_eq(class, "MultitaskingViewHost")
        || utf16_eq(class, "XamlExplorerHost")
        || (utf16_eq(class, "Windows.UI.Core.CoreWindow") && {
            let mut title_buf = [0u16; 256];
            let tlen = GetWindowTextW(hwnd, title_buf.as_mut_ptr(), 256);
            if tlen > 0 {
                title = String::from_utf16_lossy(&title_buf[..tlen as usize]);
            }
            owned_by_explorer(hwnd) || title == "MultitaskingView"
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
                overview::show_overview(&mut state.space_mgr);
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

pub(crate) unsafe extern "system" fn show_hook_proc(
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
    // OBJID_WINDOW/CHILDID_SELF does *not* mean top-level: showing any child
    // control emits this event for the control's own HWND. Every other window
    // source we listen to (EnumWindows, the ShellHook, the foreground and
    // move/size WinEvents) yields top-level handles only, so this is the one
    // place the distinction has to be made. `is_valid_window` rejects children
    // too, but this cull is a single cheap call and it keeps the great
    // majority of this very high-frequency event off the state lock entirely.
    if unsafe { GetAncestor(hwnd, GA_ROOT) } != hwnd {
        return;
    }
    with_app_state(|state| {
        if state.space_mgr.find_window(hwnd).is_some() || !spaces::is_valid_window(hwnd) {
            return;
        }
        if let Some((actual_mon, cur_space)) = state.space_mgr.adopt_at_current(hwnd) {
            winspaces_common::log_debug!(
                "EVENT_OBJECT_SHOW: tracked newly shown window {:?} to Mon {}, Space {}",
                hwnd,
                actual_mon + 1,
                cur_space + 1
            );
        }
    });
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
        if state.space_mgr.find_window(hwnd).is_none() && spaces::is_valid_window(hwnd) {
            let _ = state.space_mgr.adopt_at_current(hwnd);
        } else if state.space_mgr.tiling_enabled {
            if let Some((m_idx, s_idx)) = state.space_mgr.find_window(hwnd) {
                state.space_mgr.mark_tiling_dirty(m_idx, s_idx);
            }
        }
    });
}

/// `EVENT_OBJECT_LOCATIONCHANGE` for top-level windows. The tiler honours a
/// maximized tile as its fullscreen mode, and nothing else announces the
/// moment the user restores it (button, Win+Down): this is the only event
/// that fires then. It also fires on every frame of every drag, so the
/// filter runs before touching state, and the state lookup itself is a set
/// probe that only pays off for a window the tiler was honouring.
pub(crate) unsafe extern "system" fn location_hook_proc(
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
    if windows_sys::Win32::UI::WindowsAndMessaging::IsZoomed(hwnd) != 0 {
        return;
    }
    with_app_state(|state| {
        if state.space_mgr.tiling_enabled {
            state.space_mgr.tiling_on_window_restored(hwnd);
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
            // The tiler accepted the drag: start the modifier preview poll.
            // `SetTimer` on a live id just replaces the pending wait, so a
            // second START before an END costs nothing.
            if state.space_mgr.tiling_drag.is_some() {
                DRAG_PREVIEW_SHIFT.with(|s| s.set(false));
                SetTimer(
                    state.message_hwnd,
                    TIMER_DRAG_PREVIEW,
                    DRAG_PREVIEW_INTERVAL_MS,
                    None,
                );
            }
        } else if event == winspaces_win32::hooks::EVENT_SYSTEM_MOVESIZEEND {
            // Shift is decided before the poll state is torn down: the drop
            // commits what the preview showed (the last tick's sample), OR'd
            // with a fresh read for the drag too short to ever tick.
            let shift_held = DRAG_PREVIEW_SHIFT.with(|s| s.get()) || shift_down();
            stop_drag_preview(state.message_hwnd);
            // The drop is the one moment the daemon knows a cross-monitor move
            // was deliberate, so this — not the tiler — owns that decision;
            // `tiling_on_movesize_end` only ever classifies a gesture within
            // one monitor's layout. `RehomeTrigger::MoveSize` is the variant
            // that ignores the settle window: the guards and why are on
            // `adopt_cross_monitor_move`.
            let actual_mon_opt = state.space_mgr.monitor_index_for_hwnd(hwnd);
            let tracked_loc = state.space_mgr.find_window(hwnd);

            let rehomed = match (tracked_loc, actual_mon_opt) {
                (Some((tracked_mon, _)), Some(actual_mon)) if tracked_mon != actual_mon => {
                    state.space_mgr.adopt_cross_monitor_move(
                        hwnd,
                        tracked_mon,
                        actual_mon,
                        RehomeTrigger::MoveSize,
                    )
                }
                (None, Some(_)) if spaces::is_valid_window(hwnd) => {
                    // Finished a drag while untracked: adopt it where it
                    // landed rather than waiting for the next scan.
                    state.space_mgr.tiling_drag = None;
                    state.space_mgr.adopt_at_current(hwnd).is_some()
                }
                _ => false,
            };

            // Refusing the re-home must not also drop the gesture on the
            // floor: hand it to the tiler, which snaps the window back to its
            // tile instead of leaving it sitting on a display we declined to
            // adopt. This also keeps the pre-existing invariant that every
            // MOVESIZEEND resolves `tiling_drag`.
            if !rehomed {
                if let Some(notice) = state.space_mgr.tiling_on_movesize_end(hwnd, shift_held) {
                    winspaces_ui::space_indicator::show_split_toast(&notice);
                }
            }
        }
    });
}
