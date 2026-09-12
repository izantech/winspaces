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
    /// menu, Overview).
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

/// Query the invisible DWM drop-shadow margins of a window.
///
/// Returns `(left, top, right, bottom)` margin offsets in pixels. If DWM
/// extended frame bounds cannot be determined, falls back to `(7, 0, 7, 7)`.
///
/// Windows with custom chrome and no DWM shadow frame (e.g. WPF HwndWrapper,
/// custom-rendered apps) legitimately measure `(0, 0, 0, 0)`, which is returned
/// as-is whenever both API calls succeed.
///
/// # Safety
/// `hwnd` must be a valid window handle.
pub unsafe fn dwm_shadow_margins(hwnd: HWND) -> (i32, i32, i32, i32) {
    let mut win_rect = windows_sys::Win32::Foundation::RECT {
        left: 0,
        top: 0,
        right: 0,
        bottom: 0,
    };
    if unsafe { windows_sys::Win32::UI::WindowsAndMessaging::GetWindowRect(hwnd, &mut win_rect) }
        != 0
    {
        if let Some(frame_rect) = unsafe { extended_frame_bounds(hwnd) } {
            let l = (frame_rect.left - win_rect.left).max(0);
            let t = (frame_rect.top - win_rect.top).max(0);
            let r = (win_rect.right - frame_rect.right).max(0);
            let b = (win_rect.bottom - frame_rect.bottom).max(0);
            return (l, t, r, b);
        }
    }
    (7, 0, 7, 7)
}

/// The frame DWM actually draws for `hwnd` (`DWMWA_EXTENDED_FRAME_BOUNDS`),
/// in physical pixels: the rect the user sees, without the invisible resize
/// border that `GetWindowRect` includes. `None` when DWM has no answer for
/// the handle.
///
/// # Safety
/// `hwnd` must be a valid window handle.
pub unsafe fn extended_frame_bounds(hwnd: HWND) -> Option<windows_sys::Win32::Foundation::RECT> {
    let mut frame = windows_sys::Win32::Foundation::RECT {
        left: 0,
        top: 0,
        right: 0,
        bottom: 0,
    };
    let hr = unsafe {
        windows_sys::Win32::Graphics::Dwm::DwmGetWindowAttribute(
            hwnd,
            windows_sys::Win32::Graphics::Dwm::DWMWA_EXTENDED_FRAME_BOUNDS as _,
            &mut frame as *mut _ as _,
            std::mem::size_of::<windows_sys::Win32::Foundation::RECT>() as u32,
        )
    };
    (hr == 0).then_some(frame)
}

/// Set DWM corner rounding preference.
///
/// If `round` is true, restores default corner rounding (`DWMWCP_DEFAULT` = 0).
/// If `round` is false, disables corner rounding (`DWMWCP_DONOTROUND` = 1) for flush tiling/snapping.
///
/// # Safety
/// `hwnd` must be a valid window handle.
pub unsafe fn set_corner_rounding(hwnd: HWND, round: bool) {
    let corner_pref: u32 = if round { 0 } else { 1 };
    unsafe {
        DwmSetWindowAttribute(
            hwnd,
            DWMWA_WINDOW_CORNER_PREFERENCE,
            &corner_pref as *const _ as _,
            std::mem::size_of::<u32>() as u32,
        );
    }
}
