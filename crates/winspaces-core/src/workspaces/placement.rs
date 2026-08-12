//! Placing a window according to a captured `WorkspaceRule`.

use windows_sys::Win32::Foundation::{HWND, POINT, RECT};
use windows_sys::Win32::Graphics::Gdi::{
    GetMonitorInfoW, MonitorFromPoint, MonitorFromWindow, HMONITOR, MONITORINFO,
    MONITOR_DEFAULTTONEAREST,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    GetWindowRect, IsZoomed, SetWindowPlacement, SetWindowPos, SWP_NOACTIVATE, SWP_NOZORDER,
    SW_SHOWNOACTIVATE, WINDOWPLACEMENT,
};
use winspaces_common::{WindowRect, WorkspaceRule};

use crate::spaces::AnimationGuard;

/// Place `hwnd` according to `rule`.
///
/// `target_hmon` overrides which monitor the rule is interpreted against. The
/// default (`None`) infers it from the rect's centre, which is correct for
/// tray/IPC restores but wrong after a topology change — stale coordinates then
/// resolve to whichever monitor happens to cover them. Callers that know the
/// intended monitor by stable id pass it explicitly.
///
/// # Safety
/// `hwnd` is an opaque Win32 handle; `target_hmon`, if given, must be a
/// monitor handle from a live enumeration. The placement calls below
/// tolerate a stale or invalid `hwnd` by failing gracefully.
pub unsafe fn apply_rule_to_window(
    hwnd: HWND,
    rule: &WorkspaceRule,
    target_hmon: Option<HMONITOR>,
) {
    if rule.show_cmd == windows_sys::Win32::UI::WindowsAndMessaging::SW_SHOWMAXIMIZED as u32 {
        let mut wp: WINDOWPLACEMENT = std::mem::zeroed();
        wp.length = std::mem::size_of::<WINDOWPLACEMENT>() as u32;
        // The normal-position rect decides which monitor the window maximizes
        // onto and where it lands when un-maximized; leaving it zeroed sends
        // the window to the primary display and collapses it on restore.
        //
        // It also has to agree with `target_hmon`. Chromium and Electron apps
        // keep a degenerate restore-down rect anchored at (0,0) — observed as
        // 647x154 and 750x155 — so a maximized window whose intended monitor is
        // the secondary would otherwise always maximize onto the primary.
        let mut normal = rule.rect.clone();
        let hmon = target_hmon.unwrap_or_else(|| {
            let pt = POINT {
                x: normal.left + normal.width() / 2,
                y: normal.top + normal.height() / 2,
            };
            MonitorFromPoint(pt, MONITOR_DEFAULTTONEAREST)
        });
        let mut mi: MONITORINFO = std::mem::zeroed();
        mi.cbSize = std::mem::size_of::<MONITORINFO>() as u32;
        if GetMonitorInfoW(hmon, &mut mi) != 0 {
            normal = normal_pos_on_monitor(&normal, &mi.rcMonitor, &mi.rcWork);
        }
        wp.rcNormalPosition = RECT {
            left: normal.left,
            top: normal.top,
            right: normal.right,
            bottom: normal.bottom,
        };
        // A maximized window is glued to the monitor it is maximized on:
        // `SetWindowPlacement(SW_SHOWMAXIMIZED)` on an already-maximized
        // window only updates the restore-down rect — the OS re-picks the
        // maximize monitor solely on a restore->maximize transition. Force
        // that transition when the window sits maximized on the wrong
        // monitor, or every cross-monitor push of a maximized window is a
        // silent no-op (observed live: a maximized Brave window "pushed"
        // to the HP monitor never left the BenQ).
        // Guard bound outside the branch: it must outlive the maximize call
        // below, which is the transition that would otherwise animate.
        let _anim = (IsZoomed(hwnd) != 0
            && MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST) != hmon)
            .then(AnimationGuard::new);
        if _anim.is_some() {
            wp.showCmd = SW_SHOWNOACTIVATE as u32;
            SetWindowPlacement(hwnd, &wp);
        }
        wp.showCmd = windows_sys::Win32::UI::WindowsAndMessaging::SW_SHOWMAXIMIZED as u32;
        SetWindowPlacement(hwnd, &wp);
        return;
    }

    let pt = POINT {
        x: rule.rect.left + (rule.rect.right - rule.rect.left) / 2,
        y: rule.rect.top + (rule.rect.bottom - rule.rect.top) / 2,
    };
    let mut mi: MONITORINFO = std::mem::zeroed();
    mi.cbSize = std::mem::size_of::<MONITORINFO>() as u32;
    let hmon = target_hmon.unwrap_or_else(|| MonitorFromPoint(pt, MONITOR_DEFAULTTONEAREST));

    let (work_left, work_top, work_right, work_bottom) = if GetMonitorInfoW(hmon, &mut mi) != 0 {
        (
            mi.rcWork.left,
            mi.rcWork.top,
            mi.rcWork.right,
            mi.rcWork.bottom,
        )
    } else {
        (
            rule.rect.left,
            rule.rect.top,
            rule.rect.right,
            rule.rect.bottom,
        )
    };

    let mid_x = work_left + (work_right - work_left) / 2;
    let (is_left_half, is_right_half) =
        detect_snap_halves(rule.rect.left, rule.rect.right, work_left, work_right);
    let is_snapped = rule.is_snapped || is_left_half || is_right_half;

    let (target_l, target_t, target_r, target_b) = if is_left_half {
        (work_left, work_top, mid_x, work_bottom)
    } else if is_right_half {
        (mid_x, work_top, work_right, work_bottom)
    } else if is_snapped {
        (rule.rect.left, work_top, rule.rect.right, work_bottom)
    } else {
        (
            rule.rect.left,
            rule.rect.top,
            rule.rect.right,
            rule.rect.bottom,
        )
    };

    let mut win_rect: RECT = std::mem::zeroed();
    let mut frame_rect: RECT = std::mem::zeroed();
    let (m_left, m_top, m_right, m_bottom) = if GetWindowRect(hwnd, &mut win_rect) != 0
        && windows_sys::Win32::Graphics::Dwm::DwmGetWindowAttribute(
            hwnd,
            windows_sys::Win32::Graphics::Dwm::DWMWA_EXTENDED_FRAME_BOUNDS as _,
            &mut frame_rect as *mut _ as _,
            std::mem::size_of::<RECT>() as u32,
        ) == 0
    {
        let l = (frame_rect.left - win_rect.left).max(0);
        let t = (frame_rect.top - win_rect.top).max(0);
        let r = (win_rect.right - frame_rect.right).max(0);
        let b = (win_rect.bottom - frame_rect.bottom).max(0);
        (
            if l > 0 { l } else { 7 },
            t,
            if r > 0 { r } else { 7 },
            if b > 0 { b } else { 7 },
        )
    } else {
        (7, 0, 7, 7)
    };

    let (final_left, final_top, final_right, final_bottom) = if is_snapped {
        (
            target_l - m_left,
            target_t - m_top,
            target_r + m_right,
            target_b + m_bottom,
        )
    } else {
        (target_l, target_t, target_r, target_b)
    };

    // Apply DWM corner preference: DWMWCP_DONOTROUND (1) for snapped windows, DWMWCP_DEFAULT (0) for unsnapped
    let corner_pref: u32 = if is_snapped { 1 } else { 0 };
    windows_sys::Win32::Graphics::Dwm::DwmSetWindowAttribute(
        hwnd,
        33, // DWMWA_WINDOW_CORNER_PREFERENCE
        &corner_pref as *const _ as _,
        std::mem::size_of::<u32>() as u32,
    );

    let mut wp: WINDOWPLACEMENT = std::mem::zeroed();
    wp.length = std::mem::size_of::<WINDOWPLACEMENT>() as u32;
    wp.showCmd = rule.show_cmd;
    wp.rcNormalPosition = RECT {
        left: final_left,
        top: final_top,
        right: final_right,
        bottom: final_bottom,
    };

    SetWindowPlacement(hwnd, &wp);

    SetWindowPos(
        hwnd,
        std::ptr::null_mut(),
        final_left,
        final_top,
        final_right - final_left,
        final_bottom - final_top,
        SWP_NOZORDER
            | SWP_NOACTIVATE
            | windows_sys::Win32::UI::WindowsAndMessaging::SWP_FRAMECHANGED,
    );
}

/// Force a restore-down rect to sit on `monitor`, so `SetWindowPlacement` with
/// `SW_SHOWMAXIMIZED` maximizes onto the intended display.
///
/// Left alone when its centre is already on that monitor — that keeps the
/// un-maximize position pixel-exact in the common case. Otherwise the rect is
/// centred in the work area at its original size (clamped to fit), which
/// preserves how big the window is when the user restores it down.
pub(crate) fn normal_pos_on_monitor(rect: &WindowRect, monitor: &RECT, work: &RECT) -> WindowRect {
    let cx = rect.left + rect.width() / 2;
    let cy = rect.top + rect.height() / 2;
    let already_there =
        cx >= monitor.left && cx < monitor.right && cy >= monitor.top && cy < monitor.bottom;
    if already_there {
        return rect.clone();
    }

    let w = rect.width().clamp(1, (work.right - work.left).max(1));
    let h = rect.height().clamp(1, (work.bottom - work.top).max(1));
    let left = work.left + ((work.right - work.left) - w) / 2;
    let top = work.top + ((work.bottom - work.top) - h) / 2;
    WindowRect {
        left,
        top,
        right: left + w,
        bottom: top + h,
    }
}

/// Detect whether a window rect occupies the left or right half of the work
/// area, within the tolerance used by native snapping.
pub(crate) fn detect_snap_halves(
    rect_left: i32,
    rect_right: i32,
    work_left: i32,
    work_right: i32,
) -> (bool, bool) {
    const MARGIN: i32 = 60;
    let mid_x = work_left + (work_right - work_left) / 2;
    let near_left = (rect_left - work_left).abs() <= MARGIN;
    let near_right = (rect_right - work_right).abs() <= MARGIN;
    let left_half = near_left && (rect_right - mid_x).abs() <= MARGIN;
    let right_half = (rect_left - mid_x).abs() <= MARGIN && near_right;
    (left_half, right_half)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detect_snap_halves_recognizes_left_and_right_halves() {
        let (work_l, work_r) = (0, 1920);
        assert_eq!(detect_snap_halves(0, 960, work_l, work_r), (true, false));
        assert_eq!(detect_snap_halves(960, 1920, work_l, work_r), (false, true));
        // Within the 60px snap tolerance (DWM shadow margins).
        assert_eq!(detect_snap_halves(-7, 967, work_l, work_r), (true, false));
        // Freeform window is neither half.
        assert_eq!(detect_snap_halves(100, 800, work_l, work_r), (false, false));
        // Full-width window is neither half.
        assert_eq!(detect_snap_halves(0, 1920, work_l, work_r), (false, false));
    }

    #[test]
    fn detect_snap_halves_handles_negative_monitor_coordinates() {
        // Secondary display left of primary: work area -1536..0.
        let (work_l, work_r) = (-1536, 0);
        assert_eq!(
            detect_snap_halves(-1536, -768, work_l, work_r),
            (true, false)
        );
        assert_eq!(detect_snap_halves(-768, 0, work_l, work_r), (false, true));
    }

    fn rect(left: i32, top: i32, right: i32, bottom: i32) -> RECT {
        RECT {
            left,
            top,
            right,
            bottom,
        }
    }

    #[test]
    fn maximize_target_keeps_a_rect_already_on_the_intended_monitor() {
        // Fork's restore-down rect on the primary: must stay pixel-identical.
        let r = WindowRect {
            left: 366,
            top: 537,
            right: 2882,
            bottom: 1950,
        };
        let out = normal_pos_on_monitor(&r, &rect(0, 0, 3840, 2560), &rect(0, 0, 3840, 2508));
        assert_eq!(out, r);
    }

    #[test]
    fn maximize_target_relocates_a_degenerate_chromium_rect() {
        // Brave keeps a (0,0)-anchored 647x154 restore-down rect. Maximizing a
        // window that belongs on the secondary would otherwise land it on the
        // primary, because (0,0) is the primary's origin.
        let brave = WindowRect {
            left: 0,
            top: 0,
            right: 647,
            bottom: 154,
        };
        let hp_mon = rect(-1920, 667, 0, 1867);
        let hp_work = rect(-1920, 667, 0, 1815);
        let out = normal_pos_on_monitor(&brave, &hp_mon, &hp_work);
        let cx = out.left + out.width() / 2;
        let cy = out.top + out.height() / 2;
        assert!(
            cx >= hp_mon.left && cx < hp_mon.right,
            "cx {cx} off monitor"
        );
        assert!(
            cy >= hp_mon.top && cy < hp_mon.bottom,
            "cy {cy} off monitor"
        );
        // Restore-down size preserved.
        assert_eq!((out.width(), out.height()), (647, 154));
    }

    #[test]
    fn maximize_target_shrinks_a_rect_too_large_for_the_monitor() {
        let huge = WindowRect {
            left: 4000,
            top: 0,
            right: 7000,
            bottom: 2000,
        };
        let out = normal_pos_on_monitor(
            &huge,
            &rect(-1920, 667, 0, 1867),
            &rect(-1920, 667, 0, 1815),
        );
        assert!(out.width() <= 1920 && out.height() <= 1148);
        assert!(out.left >= -1920 && out.right <= 0);
    }
}
