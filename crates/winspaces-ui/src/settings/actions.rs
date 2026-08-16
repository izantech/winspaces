//! Control activation, post-action housekeeping (banner timer, relayout,
//! repaint), and the hotkey-recorder key handler.

use super::combo::{close_combo, open_combo, ComboKind};
use super::layout::relayout;
use super::pages::ControlId;
use super::{recorder, with_win, Win, TIMER_BANNER, TIMER_CAPTURE, WM_APP_FILE_DIALOG};
use windows_sys::Win32::Foundation::HWND;
use windows_sys::Win32::Graphics::Gdi::InvalidateRect;
use windows_sys::Win32::UI::WindowsAndMessaging::{PostMessageW, SetTimer};

/// `WM_APP_FILE_DIALOG` wparam values.
pub(crate) const FILE_DIALOG_EXPORT: usize = 0;
pub(crate) const FILE_DIALOG_IMPORT: usize = 1;

/// Alternating display/pattern pairs, double-null terminated (Win32 contract).
const CONFIG_FILTER: &str = "JSON Configuration (*.json)\0*.json\0All Files (*.*)\0*.*\0\0";

/// Export or import the config through a native file dialog.
///
/// Called from the wndproc's `WM_APP_FILE_DIALOG` arm, never from `activate`:
/// the dialog runs a nested modal loop, so it must not hold the `WIN` borrow
/// while it is up. The borrow is taken again, briefly, once a path is picked.
pub(crate) unsafe fn run_file_dialog(hwnd: HWND, kind: usize) {
    let import = kind == FILE_DIALOG_IMPORT;
    let picked = if import {
        winspaces_win32::dialogs::open_file_dialog(
            hwnd,
            CONFIG_FILTER,
            "Import WinSpaces Configuration",
        )
    } else {
        winspaces_win32::dialogs::save_file_dialog(
            hwnd,
            "winspaces-settings.json",
            CONFIG_FILTER,
            "Export WinSpaces Configuration",
        )
    };
    let Some(path) = picked else {
        return;
    };
    with_win(|win| {
        if import {
            win.state.import_from_file(&path);
        } else {
            win.state.export_to_file(&path);
        }
        unsafe { after_action(win) };
    });
}

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
        ControlId::ToggleTiling => {
            win.state.config.tiling.enabled = !win.state.config.tiling.enabled;
            win.state.autosave("Tiling enable state updated");
            after_action(win);
        }
        ControlId::ComboTheme => {
            if win
                .combo
                .as_ref()
                .is_some_and(|c| c.kind == ComboKind::Theme)
            {
                close_combo(win);
            } else {
                open_combo(win, ComboKind::Theme);
            }
        }
        ControlId::ComboInnerGap => {
            if win
                .combo
                .as_ref()
                .is_some_and(|c| c.kind == ComboKind::InnerGap)
            {
                close_combo(win);
            } else {
                open_combo(win, ComboKind::InnerGap);
            }
        }
        ControlId::ComboOuterGap => {
            if win
                .combo
                .as_ref()
                .is_some_and(|c| c.kind == ComboKind::OuterGap)
            {
                close_combo(win);
            } else {
                open_combo(win, ComboKind::OuterGap);
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
        // Both open a common dialog, which must not run under this function's
        // `WIN` borrow — see `WM_APP_FILE_DIALOG`.
        ControlId::BtnExport => {
            PostMessageW(win.hwnd, WM_APP_FILE_DIALOG, FILE_DIALOG_EXPORT, 0);
        }
        ControlId::BtnImport => {
            PostMessageW(win.hwnd, WM_APP_FILE_DIALOG, FILE_DIALOG_IMPORT, 0);
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
