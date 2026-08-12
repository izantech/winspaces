//! DPI scale helpers.
//!
//! `scale_for_window` uses `GetDpiForWindow` (the settings window and
//! Mission Control's approach, once a window exists); `scale_for_point`
//! uses `GetDpiForMonitor` against the monitor under a point (the menu's
//! approach, needed before any window exists to ask).

use windows_sys::Win32::Foundation::{HWND, POINT, RECT};
use windows_sys::Win32::Graphics::Gdi::{
    GetMonitorInfoW, MonitorFromPoint, HMONITOR, MONITORINFO, MONITOR_DEFAULTTONEAREST,
};
use windows_sys::Win32::UI::HiDpi::{GetDpiForMonitor, GetDpiForWindow, MDT_EFFECTIVE_DPI};

/// DPI scale of the monitor `hwnd` is on, relative to 96 dpi (100%). Never
/// below 1.0.
///
/// # Safety
/// `hwnd` must be a valid window handle.
pub unsafe fn scale_for_window(hwnd: HWND) -> f32 {
    let dpi = unsafe { GetDpiForWindow(hwnd) };
    if dpi > 0 {
        (dpi as f32 / 96.0).max(1.0)
    } else {
        1.0
    }
}

/// DPI scale of the monitor nearest `pt`, relative to 96 dpi (100%). Used
/// before a window exists to ask `GetDpiForWindow` — the menu resolves this
/// from the point that will anchor it.
///
/// # Safety
/// Thin FFI wrapper; `pt` is a plain value, no handle safety requirements.
pub unsafe fn scale_for_point(pt: POINT) -> f32 {
    let hmon = unsafe { MonitorFromPoint(pt, MONITOR_DEFAULTTONEAREST) };
    let mut dpi_x: u32 = 96;
    let mut dpi_y: u32 = 96;
    unsafe {
        GetDpiForMonitor(hmon, MDT_EFFECTIVE_DPI, &mut dpi_x, &mut dpi_y);
    }
    (dpi_x.max(96) as f32) / 96.0
}

/// Scale a 96-dpi design-unit value to device pixels.
pub fn px(scale: f32, v: i32) -> i32 {
    (v as f32 * scale).round() as i32
}

/// The work area (screen rect minus the taskbar) of monitor `hmon`, or
/// `None` if `GetMonitorInfoW` fails.
///
/// # Safety
/// `hmon` must be a valid monitor handle (e.g. from `MonitorFromPoint` /
/// `MonitorFromWindow`).
pub unsafe fn work_area(hmon: HMONITOR) -> Option<RECT> {
    let mut mi: MONITORINFO = unsafe { std::mem::zeroed() };
    mi.cbSize = std::mem::size_of::<MONITORINFO>() as u32;
    if unsafe { GetMonitorInfoW(hmon, &mut mi) } == 0 {
        return None;
    }
    Some(mi.rcWork)
}
