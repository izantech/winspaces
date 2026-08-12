//! Per-display metrics: refresh-driven frame budget and the raw monitor
//! rectangle (as opposed to `dpi::work_area`'s taskbar-excluded rect).

use windows_sys::Win32::Foundation::{HWND, RECT};
use windows_sys::Win32::Graphics::Gdi::{
    GetDC, GetDeviceCaps, GetMonitorInfoW, ReleaseDC, HMONITOR, MONITORINFO, VREFRESH,
};

/// The display's frame interval in milliseconds — the budget a single
/// repaint should stay inside of. Falls back to a 60 Hz assumption when the
/// driver reports 0 or 1 (both mean "hardware default").
///
/// # Safety
/// `hwnd` must be null or a valid window handle.
pub unsafe fn frame_interval_ms(hwnd: HWND) -> u32 {
    let hdc = unsafe { GetDC(hwnd) };
    if hdc.is_null() {
        return 16;
    }
    let hz = unsafe { GetDeviceCaps(hdc, VREFRESH as i32) };
    unsafe {
        ReleaseDC(hwnd, hdc);
    }
    // 0 and 1 both mean "hardware default" — assume the 60 Hz floor.
    let hz = if hz <= 1 { 60 } else { hz.min(360) };
    ((1000 / hz) as u32).max(4)
}

/// The full rectangle (not the work area) of monitor `hmon`, or `None` if
/// `GetMonitorInfoW` fails.
///
/// # Safety
/// `hmon` must be a valid monitor handle (e.g. from `MonitorFromPoint` /
/// `MonitorFromWindow`).
pub unsafe fn monitor_rect_of(hmon: HMONITOR) -> Option<RECT> {
    let mut mi: MONITORINFO = unsafe { std::mem::zeroed() };
    mi.cbSize = std::mem::size_of::<MONITORINFO>() as u32;
    if unsafe { GetMonitorInfoW(hmon, &mut mi) } == 0 {
        return None;
    }
    Some(mi.rcMonitor)
}
