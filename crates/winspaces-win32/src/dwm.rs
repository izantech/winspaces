//! DWM window attributes: dark-mode titlebar cue, rounded corners, the
//! "sheet of glass" frame extension, and backdrop material.

use windows_sys::Win32::Foundation::HWND;
use windows_sys::Win32::Graphics::Dwm::{
    DwmExtendFrameIntoClientArea, DwmSetWindowAttribute, DWMWA_SYSTEMBACKDROP_TYPE,
    DWMWA_USE_IMMERSIVE_DARK_MODE,
};
use windows_sys::Win32::UI::Controls::MARGINS;

/// windows-sys 0.59 does not export `DWMWA_WINDOW_CORNER_PREFERENCE`.
const DWMWA_WINDOW_CORNER_PREFERENCE: u32 = 33;
/// `DWMWCP_ROUND` — Win11 rounds the window and supplies its own shadow.
const DWMWCP_ROUND: u32 = 2;

/// Backdrop material for `set_backdrop`.
pub enum Backdrop {
    /// `DWMSBT_MAINWINDOW` — the settings window's long-lived frosted look.
    Mica,
    /// `DWMSBT_TRANSIENTWINDOW` — the flyout/overlay material (the tray
    /// menu, Mission Control).
    Acrylic,
}

/// Steer `DWMWA_USE_IMMERSIVE_DARK_MODE`, which also tints the acrylic/Mica
/// backdrop and the default titlebar.
///
/// # Safety
/// `hwnd` must be a valid window handle.
pub unsafe fn set_dark_mode(hwnd: HWND, dark: bool) {
    let value: i32 = if dark { 1 } else { 0 };
    unsafe {
        DwmSetWindowAttribute(
            hwnd,
            DWMWA_USE_IMMERSIVE_DARK_MODE as _,
            &value as *const _ as _,
            std::mem::size_of::<i32>() as u32,
        );
    }
}

/// Round the window's corners (`DWMWCP_ROUND`).
///
/// # Safety
/// `hwnd` must be a valid window handle.
pub unsafe fn set_round_corners(hwnd: HWND) {
    let corner = DWMWCP_ROUND;
    unsafe {
        DwmSetWindowAttribute(
            hwnd,
            DWMWA_WINDOW_CORNER_PREFERENCE,
            &corner as *const _ as _,
            std::mem::size_of::<u32>() as u32,
        );
    }
}

/// Extend the DWM frame into the entire client area ("sheet of glass").
///
/// **Load-bearing order**: this must be called *before* `set_backdrop` has
/// any visible effect. Kept as a separate function from `set_backdrop` on
/// purpose — do not fold the two into one convenience call someone could
/// reorder internally; call sites must call them in this order themselves.
///
/// # Safety
/// `hwnd` must be a valid window handle.
pub unsafe fn extend_frame_full(hwnd: HWND) {
    let margins = MARGINS {
        cxLeftWidth: -1,
        cxRightWidth: -1,
        cyTopHeight: -1,
        cyBottomHeight: -1,
    };
    unsafe {
        DwmExtendFrameIntoClientArea(hwnd, &margins);
    }
}

/// Set the DWM backdrop material. Call `extend_frame_full` first — see its
/// doc comment.
///
/// # Safety
/// `hwnd` must be a valid window handle.
pub unsafe fn set_backdrop(hwnd: HWND, backdrop: Backdrop) {
    let value: i32 = match backdrop {
        Backdrop::Mica => 2,    // DWMSBT_MAINWINDOW
        Backdrop::Acrylic => 3, // DWMSBT_TRANSIENTWINDOW
    };
    unsafe {
        DwmSetWindowAttribute(
            hwnd,
            DWMWA_SYSTEMBACKDROP_TYPE as _,
            &value as *const _ as _,
            std::mem::size_of::<i32>() as u32,
        );
    }
}
