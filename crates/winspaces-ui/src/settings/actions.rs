//! Control activation, post-action housekeeping (banner timer, relayout,
//! repaint), and the hotkey-recorder key handler.

use super::combo::{close_combo, open_combo};
use super::layout::relayout;
use super::pages::ControlId;
use super::{recorder, Win, TIMER_BANNER, TIMER_CAPTURE};
use windows_sys::Win32::Graphics::Gdi::InvalidateRect;
use windows_sys::Win32::UI::WindowsAndMessaging::SetTimer;

/// Post-action housekeeping: banner timer, relayout (banner/content changes
/// shift rects), repaint.
pub(crate) unsafe fn after_action(win: &mut Win) {
    if win.state.banner.is_some() {
        SetTimer(win.hwnd, TIMER_BANNER, 4000, None);
    }
    relayout(win);
    InvalidateRect(win.hwnd, std::ptr::null(), 0);
}

pub(crate) unsafe fn stop_recording(win: &mut Win) {
    if win.state.capturing.is_some() {
        recorder::stop_capture();
        win.state.capturing = None;
        InvalidateRect(win.hwnd, std::ptr::null(), 0);
    }
}

pub(crate) unsafe fn activate(win: &mut Win, id: ControlId) {
    // Any activation while recording cancels the recording first.
    if win.state.capturing.is_some() && !matches!(id, ControlId::Hotkey(_)) {
        stop_recording(win);
    }
    match id {
        ControlId::Nav(page) | ControlId::NavCard(page, _) => {
            if win.state.page != page {
                win.state.page = page;
                win.scroll = 0;
                win.hover = None;
                relayout(win);
                InvalidateRect(win.hwnd, std::ptr::null(), 0);
            }
        }
        ControlId::ToggleShowAll => {
            win.state.config.show_all_taskbar = !win.state.config.show_all_taskbar;
            win.state.autosave("Taskbar visibility mode updated");
            after_action(win);
        }
        ControlId::ToggleWinTab => {
            win.state.config.intercept_win_tab = !win.state.config.intercept_win_tab;
            win.state.autosave("Intercept Win + Tab preference updated");
            after_action(win);
        }
        ControlId::ToggleSpaceIndicator => {
            win.state.config.space_indicator = !win.state.config.space_indicator;
            win.state.autosave("Space indicator preference updated");
            after_action(win);
        }
        ControlId::ToggleAutoRestore => {
            win.state.config.auto_restore_workspaces = !win.state.config.auto_restore_workspaces;
            win.state.autosave("Auto-restore preference updated");
            after_action(win);
        }
        ControlId::ToggleAutostart => {
            win.state.toggle_autostart();
            after_action(win);
        }
        ControlId::ComboTheme => {
            if win.combo.is_some() {
                close_combo(win);
            } else {
                open_combo(win);
            }
        }
        ControlId::BtnReload => {
            win.state.refresh_daemon_status();
            relayout(win);
            InvalidateRect(win.hwnd, std::ptr::null(), 0);
        }
        ControlId::BtnCapture => {
            if win.state.request_capture() {
                // Capture is asynchronous: give the daemon ~300ms to write
                // the captured rules, then re-read the file.
                SetTimer(win.hwnd, TIMER_CAPTURE, 300, None);
            } else {
                win.state.show_banner(
                    "Daemon Not Running",
                    "Start the WinSpaces daemon to capture window layouts.",
                    false,
                );
                after_action(win);
            }
        }
        ControlId::BtnRestore => {
            win.state.request_restore();
            after_action(win);
        }
        ControlId::BtnReset => {
            win.state.reset_defaults();
            after_action(win);
        }
        ControlId::Hotkey(target) => {
            if win.state.capturing == Some(target) {
                stop_recording(win);
            } else {
                recorder::stop_capture();
                win.state.capture_hook = recorder::start_capture(win.hwnd);
                win.state.capturing = Some(target);
                InvalidateRect(win.hwnd, std::ptr::null(), 0);
            }
        }
        ControlId::RuleDelete(index) => {
            win.state.delete_rule(index);
            after_action(win);
        }
    }
}

pub(crate) unsafe fn handle_recorder_key(win: &mut Win, vk: u32) {
    let Some(target) = win.state.capturing else {
        return;
    };
    match recorder::translate_current(vk) {
        recorder::Outcome::Ignore => {}
        recorder::Outcome::Cancel => {
            stop_recording(win);
        }
        recorder::Outcome::Commit(hk) => {
            recorder::stop_capture();
            win.state.capturing = None;
            win.state.set_hotkey(target, hk);
            after_action(win);
        }
    }
}
