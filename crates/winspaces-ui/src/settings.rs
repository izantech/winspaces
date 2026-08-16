//! Native settings window (`winspaces.exe --settings`). Runs in its own
//! process instance of the daemon binary — `run_settings` is only reachable
//! from the `--settings` CLI branch, so the daemon itself never touches any
//! of this state.
//!
//! Rendering is the menu.rs technique writ large: one `WS_OVERLAPPEDWINDOW`
//! with a real Mica backdrop (`DWMWA_SYSTEMBACKDROP_TYPE`), painted per frame
//! into a premultiplied 32bpp DIB with a semi-transparent theme tint so the
//! backdrop shows through. Every control is an owner-drawn region of this one
//! window; the only child surface is the theme combo's `WS_EX_NOACTIVATE`
//! dropdown popup. See docs/settings-ui.md.

mod actions;
mod autostart;
mod combo;
mod controls;
mod layout;
mod pages;
mod recorder;
mod render;
mod state;
mod wndproc;

use crate::theme::{self, Palette};
use combo::ComboPopup;
use controls::{Fonts, Vis};
use layout::relayout;
use pages::Layout;
use state::SettingsState;
use std::cell::RefCell;
use std::ptr::null_mut;

use windows_sys::Win32::Foundation::HWND;
use windows_sys::Win32::Foundation::RECT;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, FindWindowW, GetClientRect, GetSystemMetrics, SetForegroundWindow,
    SetWindowPos, ShowWindow, CW_USEDEFAULT, MSG, SM_CXSCREEN, SM_CYSCREEN, SWP_NOACTIVATE,
    SWP_NOZORDER, SW_RESTORE, SW_SHOW, WS_OVERLAPPEDWINDOW,
};

const WM_MOUSELEAVE: u32 = 0x02A3;

use winspaces_common::log_info;
use winspaces_win32::dpi;
use winspaces_win32::dwm::{extend_frame_full, set_backdrop, set_dark_mode, Backdrop};
use winspaces_win32::module::{app_instance, win_build};
use winspaces_win32::text::encode_wide;
use winspaces_win32::window_class::register_class;

const SETTINGS_CLASS: &str = "WinSpacesSettingsClass";
const SETTINGS_TITLE: &str = "WinSpaces Settings";
const POPUP_CLASS: &str = "WinSpacesSettingsPopup";

/// Combo popup commits a selection: wparam = ThemePref index.
const WM_APP_COMBO_COMMIT: u32 = windows_sys::Win32::UI::WindowsAndMessaging::WM_APP + 71;

/// Run the export/import file dialog: wparam = 0 for export, 1 for import.
///
/// Posted rather than called inline because a common dialog spins its own
/// modal message loop, and `activate` runs *inside* `with_win`'s `borrow_mut`.
/// Every message that loop pumps back to us — `WM_PAINT` above all — hits a
/// `try_borrow` that fails while the dialog is up, so the window would sit
/// there unable to redraw the regions the dialog uncovers. Deferring to the
/// message loop puts the dialog outside the borrow entirely.
const WM_APP_FILE_DIALOG: u32 = windows_sys::Win32::UI::WindowsAndMessaging::WM_APP + 72;

const WM_DPICHANGED: u32 = 0x02E0;

const TIMER_BANNER: usize = 1;
const TIMER_CAPTURE: usize = 2;

/// Nav rail metrics (96-dpi dips).
const NAV_W: i32 = 240;
const NAV_ITEM_H: i32 = 40;
const NAV_ITEM_GAP: i32 = 4;

struct Win {
    hwnd: HWND,
    scale: f32,
    fonts: Fonts,
    pal: Palette,
    mica: bool,
    state: SettingsState,
    layout: Layout,
    nav_rects: [RECT; 3],
    scroll: i32,
    hover: Option<pages::ControlId>,
    pressed: Option<pages::ControlId>,
    /// Index into the focus order (nav items then content controls).
    focus: Option<usize>,
    /// Focus rings only show for keyboard-driven focus (Win11 behavior).
    keyboard_nav: bool,
    combo: Option<ComboPopup>,
    tracking_leave: bool,
    /// Offset between the grab point and the thumb top while dragging.
    scrollbar_drag: Option<i32>,
    client_w: i32,
    client_h: i32,
}

thread_local! {
    static WIN: RefCell<Option<Win>> = const { RefCell::new(None) };
}

fn with_win<F: FnOnce(&mut Win)>(f: F) {
    WIN.with(|s| {
        if let Ok(mut borrow) = s.try_borrow_mut() {
            if let Some(win) = borrow.as_mut() {
                f(win);
            }
        }
    });
}

/// Entry point for `winspaces.exe --settings`. Blocks until the window closes.
pub fn run_settings() {
    unsafe {
        // Single settings instance: focus the existing window instead.
        let class_name = encode_wide(SETTINGS_CLASS);
        let title = encode_wide(SETTINGS_TITLE);
        let existing = FindWindowW(class_name.as_ptr(), title.as_ptr());
        if !existing.is_null() {
            ShowWindow(existing, SW_RESTORE);
            SetForegroundWindow(existing);
            return;
        }

        let state = SettingsState::new();
        let mica = win_build() >= 22621;
        let pal = theme::build_palette(state.theme_pref, mica);

        register_class(SETTINGS_CLASS, Some(wndproc::settings_wnd_proc));
        register_class(POPUP_CLASS, Some(combo::popup_wnd_proc));

        let hwnd = CreateWindowExW(
            0,
            class_name.as_ptr(),
            title.as_ptr(),
            WS_OVERLAPPEDWINDOW,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            960,
            720,
            null_mut(),
            null_mut(),
            app_instance(),
            null_mut(),
        );
        if hwnd.is_null() {
            log_info!("Failed to create settings window");
            return;
        }

        let scale = dpi::scale_for_window(hwnd);

        // 960x720 dips, centered on the primary monitor.
        let w = (960.0 * scale).round() as i32;
        let h = (720.0 * scale).round() as i32;
        let sw = GetSystemMetrics(SM_CXSCREEN);
        let sh = GetSystemMetrics(SM_CYSCREEN);
        SetWindowPos(
            hwnd,
            null_mut(),
            ((sw - w) / 2).max(0),
            ((sh - h) / 2).max(0),
            w,
            h,
            SWP_NOZORDER | SWP_NOACTIVATE,
        );

        apply_frame_attributes(hwnd, &pal, mica);

        let fonts = controls::create_fonts(scale);
        let mut win = Win {
            hwnd,
            scale,
            fonts,
            pal,
            mica,
            state,
            layout: Layout {
                items: Vec::new(),
                content_h: 0,
                controls: Vec::new(),
            },
            nav_rects: [RECT {
                left: 0,
                top: 0,
                right: 0,
                bottom: 0,
            }; 3],
            scroll: 0,
            hover: None,
            pressed: None,
            focus: None,
            keyboard_nav: false,
            combo: None,
            tracking_leave: false,
            scrollbar_drag: None,
            client_w: 0,
            client_h: 0,
        };
        let mut rc: RECT = std::mem::zeroed();
        GetClientRect(hwnd, &mut rc);
        win.client_w = rc.right;
        win.client_h = rc.bottom;
        relayout(&mut win);

        WIN.with(|s| *s.borrow_mut() = Some(win));

        ShowWindow(hwnd, SW_SHOW);
        SetForegroundWindow(hwnd);

        let mut msg: MSG = std::mem::zeroed();
        while windows_sys::Win32::UI::WindowsAndMessaging::GetMessageW(&mut msg, null_mut(), 0, 0)
            > 0
        {
            windows_sys::Win32::UI::WindowsAndMessaging::TranslateMessage(&msg);
            windows_sys::Win32::UI::WindowsAndMessaging::DispatchMessageW(&msg);
        }

        // Teardown (WM_DESTROY already stopped any capture hook).
        let win = WIN.with(|s| s.borrow_mut().take());
        if let Some(win) = win {
            controls::delete_fonts(&win.fonts);
        }
    }
}

unsafe fn apply_frame_attributes(hwnd: HWND, pal: &Palette, mica: bool) {
    set_dark_mode(hwnd, !pal.light);
    if mica && !pal.high_contrast {
        // Load-bearing order: extend_frame_full before set_backdrop.
        extend_frame_full(hwnd);
        set_backdrop(hwnd, Backdrop::Mica);
    }
}
