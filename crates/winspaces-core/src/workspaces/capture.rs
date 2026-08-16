//! Capturing the live session as a `Vec<WorkspaceRule>`.

use windows_sys::Win32::Foundation::{HWND, POINT};
use windows_sys::Win32::Graphics::Gdi::{
    GetMonitorInfoW, MonitorFromPoint, MONITORINFO, MONITOR_DEFAULTTONEAREST,
};
use windows_sys::Win32::UI::WindowsAndMessaging::GetWindowThreadProcessId;
use winspaces_common::WorkspaceRule;

use super::identity::window_identity;
use super::placement::detect_snap_halves;
use super::query::{get_window_placement_info, get_window_title};
use crate::spaces::{is_valid_window, SpaceManager};

unsafe fn capture_window(
    hwnd: HWND,
    mon_idx: usize,
    space_idx: usize,
    is_sticky: bool,
) -> WorkspaceRule {
    let identity = window_identity(hwnd);
    let (aumid, exe_path, class_name) = (identity.aumid, identity.exe_path, identity.class_name);
    let title = get_window_title(hwnd);
    let (show_cmd, rect) = get_window_placement_info(hwnd);

    let exe_name = std::path::Path::new(&exe_path)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("App");

    let name = if !aumid.is_empty() {
        if !title.is_empty() {
            format!("{} [{}]", title, aumid)
        } else {
            format!("{} [{}]", exe_name, aumid)
        }
    } else if !title.is_empty() {
        format!("{} ({})", exe_name, title)
    } else {
        exe_name.to_string()
    };

    let title_pattern = if title.is_empty() {
        String::new()
    } else {
        title.clone()
    };

    let pt = POINT {
        x: rect.left + (rect.right - rect.left) / 2,
        y: rect.top + (rect.bottom - rect.top) / 2,
    };
    let mut mi: MONITORINFO = std::mem::zeroed();
    mi.cbSize = std::mem::size_of::<MONITORINFO>() as u32;
    let hmon = MonitorFromPoint(pt, MONITOR_DEFAULTTONEAREST);
    let (work_left, work_right) = if GetMonitorInfoW(hmon, &mut mi) != 0 {
        (mi.rcWork.left, mi.rcWork.right)
    } else {
        (rect.left, rect.right)
    };
    let (is_left_half, is_right_half) =
        detect_snap_halves(rect.left, rect.right, work_left, work_right);
    let is_snapped = is_left_half || is_right_half;

    WorkspaceRule {
        name,
        aumid,
        exe_path,
        class_name,
        title_pattern,
        display_index: mon_idx,
        space_index: space_idx,
        show_cmd,
        rect,
        is_snapped,
        is_sticky,
    }
}

/// One captured window: the rule plus the live handle and owning process it
/// was captured from, so a same-session restore can re-identify the *exact*
/// window instead of guessing by name.
pub struct CapturedWindow {
    pub hwnd: HWND,
    pub pid: u32,
    pub rule: WorkspaceRule,
}

/// Walks the manager's own tracked set rather than `EnumWindows`: untracked
/// windows never contributed to the capture (the old enumeration discarded
/// every `find_window` miss), and the tracked set already carries each
/// window's (monitor, space) position. Ordering differences don't matter —
/// every consumer either sorts canonically or is order-independent.
///
/// The `is_valid_window` filter is load-bearing: a tracked window that is
/// currently ineligible (e.g. externally cloaked) must stay out of the
/// capture, or the snapshot's window count changes meaning.
///
/// # Safety
/// Calls raw Win32 window queries; must be called from a thread that may
/// legally query top-level windows (any UI or worker thread).
pub unsafe fn capture_active_workspace_detailed(mgr: &SpaceManager) -> Vec<CapturedWindow> {
    let mut metas: Vec<(HWND, u32)> = Vec::new();
    let mut rules: Vec<WorkspaceRule> = Vec::new();
    for (mon_idx, mon) in mgr.monitors.iter().enumerate() {
        for (space_idx, space) in mon.spaces.iter().enumerate() {
            for &hwnd in space {
                if is_valid_window(hwnd) {
                    let mut pid: u32 = 0;
                    GetWindowThreadProcessId(hwnd, &mut pid);
                    metas.push((hwnd, pid));
                    rules.push(capture_window(
                        hwnd,
                        mon_idx,
                        space_idx,
                        mgr.is_sticky(hwnd),
                    ));
                }
            }
        }
    }
    drop_redundant_title_patterns(&mut rules);
    metas
        .into_iter()
        .zip(rules)
        .map(|((hwnd, pid), rule)| CapturedWindow { hwnd, pid, rule })
        .collect()
}

/// # Safety
/// See [`capture_active_workspace_detailed`].
pub unsafe fn capture_active_workspace(mgr: &SpaceManager) -> Vec<WorkspaceRule> {
    capture_active_workspace_detailed(mgr)
        .into_iter()
        .map(|c| c.rule)
        .collect()
}

/// Window titles are volatile (page navigation, unread counters, open file),
/// and a rule with a `title_pattern` is disqualified when the title no longer
/// matches — breaking restore for the common case. Keep the captured title as
/// a matcher only when several rules share the same app identity (AUMID + exe)
/// and the title is the only way to tell the windows apart.
pub(crate) fn drop_redundant_title_patterns(rules: &mut [WorkspaceRule]) {
    let keys: Vec<(String, String)> = rules
        .iter()
        .map(|r| (r.aumid.to_lowercase(), r.exe_path.to_lowercase()))
        .collect();
    for i in 0..rules.len() {
        let ambiguous = keys
            .iter()
            .enumerate()
            .any(|(j, key)| j != i && *key == keys[i]);
        if !ambiguous {
            rules[i].title_pattern = String::new();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn title_patterns_dropped_when_app_identity_is_unique() {
        let mut rules = vec![
            WorkspaceRule {
                exe_path: r"C:\apps\telegram.exe".into(),
                title_pattern: "Telegram (2)".into(),
                ..Default::default()
            },
            WorkspaceRule {
                exe_path: r"C:\apps\brave.exe".into(),
                aumid: "BravePWA.WhatsApp".into(),
                title_pattern: "(3) WhatsApp Web".into(),
                ..Default::default()
            },
        ];
        drop_redundant_title_patterns(&mut rules);
        assert!(rules[0].title_pattern.is_empty());
        assert!(rules[1].title_pattern.is_empty());
    }

    #[test]
    fn title_patterns_kept_for_same_app_multiple_windows() {
        let mut rules = vec![
            WorkspaceRule {
                exe_path: r"C:\apps\chrome.exe".into(),
                title_pattern: "Gmail".into(),
                ..Default::default()
            },
            WorkspaceRule {
                exe_path: r"C:\apps\chrome.exe".into(),
                title_pattern: "Calendar".into(),
                ..Default::default()
            },
        ];
        drop_redundant_title_patterns(&mut rules);
        assert_eq!(rules[0].title_pattern, "Gmail");
        assert_eq!(rules[1].title_pattern, "Calendar");
    }
}
