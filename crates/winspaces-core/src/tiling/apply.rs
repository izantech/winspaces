//! Batch window position application using DeferWindowPos and DWM margin compensation.

use std::ptr::null_mut;
use windows_sys::Win32::Foundation::HWND;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    BeginDeferWindowPos, DeferWindowPos, EndDeferWindowPos, SetWindowPos, SWP_FRAMECHANGED,
    SWP_NOACTIVATE, SWP_NOCOPYBITS, SWP_NOZORDER,
};
use winspaces_common::{log_warn, WindowRect};

const TILE_SWP_FLAGS: u32 = SWP_NOZORDER | SWP_NOACTIVATE | SWP_FRAMECHANGED | SWP_NOCOPYBITS;

/// Apply target bounding rectangles to a set of windows in a single atomic DeferWindowPos batch.
///
/// Compensates for invisible DWM drop-shadow margins so windows tile flush to each other,
/// and applies `DWMWCP_DONOTROUND` for square-edge flush tiling.
///
/// If DeferWindowPos fails (e.g. window destroyed mid-retile), the batch handle is invalidated
/// by the OS, so we abandon the batch and fall back to sequential SetWindowPos for remaining windows.
pub fn apply_layout(placements: &[(HWND, WindowRect)]) {
    if placements.is_empty() {
        return;
    }

    unsafe {
        let mut hdwp = BeginDeferWindowPos(placements.len() as i32);
        if hdwp.is_null() {
            log_warn!("BeginDeferWindowPos failed; falling back to sequential SetWindowPos");
            for &(hwnd, ref target) in placements {
                apply_single_placement(hwnd, target);
            }
            return;
        }

        for &(hwnd, ref target) in placements {
            let (m_l, m_t, m_r, m_b) = winspaces_win32::dwm::dwm_shadow_margins(hwnd);
            winspaces_win32::dwm::set_corner_rounding(hwnd, false);

            let x = target.left - m_l;
            let y = target.top - m_t;
            let cx = target.width() + m_l + m_r;
            let cy = target.height() + m_t + m_b;

            let next_hdwp = DeferWindowPos(hdwp, hwnd, null_mut(), x, y, cx, cy, TILE_SWP_FLAGS);

            if next_hdwp.is_null() {
                log_warn!(
                    "DeferWindowPos failed for hwnd {:?}; falling back to sequential SetWindowPos for all windows",
                    hwnd
                );
                for &(rem_hwnd, ref rem_target) in placements {
                    apply_single_placement(rem_hwnd, rem_target);
                }
                return;
            }

            hdwp = next_hdwp;
        }

        EndDeferWindowPos(hdwp);
    }
}

unsafe fn apply_single_placement(hwnd: HWND, target: &WindowRect) {
    let (m_l, m_t, m_r, m_b) = winspaces_win32::dwm::dwm_shadow_margins(hwnd);
    winspaces_win32::dwm::set_corner_rounding(hwnd, false);
    SetWindowPos(
        hwnd,
        null_mut(),
        target.left - m_l,
        target.top - m_t,
        target.width() + m_l + m_r,
        target.height() + m_t + m_b,
        TILE_SWP_FLAGS,
    );
}
