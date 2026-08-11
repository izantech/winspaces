use crate::desktop::{is_valid_window, DesktopManager};
use windows_sys::Win32::Foundation::{CloseHandle, HANDLE, HWND, POINT, RECT};
use windows_sys::Win32::Graphics::Gdi::{
    GetMonitorInfoW, MonitorFromPoint, HMONITOR, MONITORINFO, MONITOR_DEFAULTTONEAREST,
};
use windows_sys::Win32::System::Threading::{
    OpenProcess, QueryFullProcessImageNameW, PROCESS_QUERY_LIMITED_INFORMATION,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GetClassNameW, GetWindowPlacement, GetWindowRect, GetWindowTextW,
    GetWindowThreadProcessId, IsWindowVisible, SetWindowPlacement, SetWindowPos, SWP_NOACTIVATE,
    SWP_NOZORDER, WINDOWPLACEMENT,
};
use winspaces_common::{WindowRect, WorkspaceRule};

pub unsafe fn get_process_image_path(hwnd: HWND) -> String {
    let mut pid = 0;
    GetWindowThreadProcessId(hwnd, &mut pid);
    if pid == 0 {
        return String::new();
    }

    let h_process: HANDLE = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
    if h_process.is_null() {
        return String::new();
    }

    let mut buf = [0u16; 1024];
    let mut size = buf.len() as u32;
    let mut path = String::new();

    if QueryFullProcessImageNameW(h_process, 0, buf.as_mut_ptr(), &mut size) != 0 {
        path = String::from_utf16_lossy(&buf[..size as usize]);
    }

    CloseHandle(h_process);
    path
}

pub unsafe fn get_window_class(hwnd: HWND) -> String {
    let mut buf = [0u16; 256];
    let len = GetClassNameW(hwnd, buf.as_mut_ptr(), buf.len() as i32);
    if len > 0 {
        String::from_utf16_lossy(&buf[..len as usize])
    } else {
        String::new()
    }
}

#[allow(non_camel_case_types, clippy::upper_case_acronyms)]
#[repr(C)]
struct PROPERTYKEY {
    fmtid: windows_sys::core::GUID,
    pid: u32,
}

#[allow(non_camel_case_types, clippy::upper_case_acronyms)]
#[repr(C)]
struct PROPVARIANT {
    vt: u16,
    w_reserved1: u16,
    w_reserved2: u16,
    w_reserved3: u16,
    val1: usize,
    val2: usize,
}

#[allow(non_snake_case)]
#[repr(C)]
struct IPropertyStoreVtbl {
    pub QueryInterface: unsafe extern "system" fn(
        this: *mut std::ffi::c_void,
        riid: *const windows_sys::core::GUID,
        ppv: *mut *mut std::ffi::c_void,
    ) -> i32,
    pub AddRef: unsafe extern "system" fn(this: *mut std::ffi::c_void) -> u32,
    pub Release: unsafe extern "system" fn(this: *mut std::ffi::c_void) -> u32,
    pub GetCount: unsafe extern "system" fn(this: *mut std::ffi::c_void, cprops: *mut u32) -> i32,
    pub GetAt: unsafe extern "system" fn(
        this: *mut std::ffi::c_void,
        iprop: u32,
        pkey: *mut PROPERTYKEY,
    ) -> i32,
    pub GetValue: unsafe extern "system" fn(
        this: *mut std::ffi::c_void,
        key: *const PROPERTYKEY,
        pv: *mut PROPVARIANT,
    ) -> i32,
    pub SetValue: unsafe extern "system" fn(
        this: *mut std::ffi::c_void,
        key: *const PROPERTYKEY,
        pv: *const PROPVARIANT,
    ) -> i32,
    pub Commit: unsafe extern "system" fn(this: *mut std::ffi::c_void) -> i32,
}

#[link(name = "shell32")]
extern "system" {
    fn SHGetPropertyStoreForWindow(
        hwnd: HWND,
        riid: *const windows_sys::core::GUID,
        ppv: *mut *mut std::ffi::c_void,
    ) -> i32;
}

use windows_sys::Win32::System::Com::CoTaskMemFree;

pub unsafe fn get_window_aumid(hwnd: HWND) -> String {
    const IID_IPROPERTYSTORE: windows_sys::core::GUID = windows_sys::core::GUID {
        data1: 0x886d8eeb,
        data2: 0x8cf2,
        data3: 0x4446,
        data4: [0x8d, 0x02, 0xcd, 0xba, 0x1d, 0xbd, 0xcf, 0x99],
    };

    const PKEY_APPUSERMODEL_ID: PROPERTYKEY = PROPERTYKEY {
        fmtid: windows_sys::core::GUID {
            data1: 0x9F4C2855,
            data2: 0x9F79,
            data3: 0x4B39,
            data4: [0xA8, 0xD0, 0xE1, 0xD4, 0x2D, 0xE1, 0xD5, 0xF3],
        },
        pid: 5,
    };

    let mut store_ptr: *mut std::ffi::c_void = std::ptr::null_mut();
    if SHGetPropertyStoreForWindow(hwnd, &IID_IPROPERTYSTORE, &mut store_ptr) == 0
        && !store_ptr.is_null()
    {
        let vtbl = *(store_ptr as *mut *const IPropertyStoreVtbl);
        let mut pv: PROPVARIANT = std::mem::zeroed();
        let mut result = String::new();
        if ((*vtbl).GetValue)(store_ptr, &PKEY_APPUSERMODEL_ID, &mut pv) == 0
            && pv.vt == 31
            && pv.val1 != 0
        {
            // VT_LPWSTR = 31
            let ptr = pv.val1 as *const u16;
            let mut len = 0;
            while *ptr.add(len) != 0 {
                len += 1;
            }
            let slice = std::slice::from_raw_parts(ptr, len);
            result = String::from_utf16_lossy(slice);
            CoTaskMemFree(ptr as *const std::ffi::c_void);
        }
        ((*vtbl).Release)(store_ptr);
        return result;
    }

    String::new()
}

pub unsafe fn get_window_title(hwnd: HWND) -> String {
    let mut buf = [0u16; 512];
    let len = GetWindowTextW(hwnd, buf.as_mut_ptr(), buf.len() as i32);
    if len > 0 {
        String::from_utf16_lossy(&buf[..len as usize])
    } else {
        String::new()
    }
}

pub unsafe fn get_window_placement_info(hwnd: HWND) -> (u32, WindowRect) {
    let mut wp: WINDOWPLACEMENT = std::mem::zeroed();
    wp.length = std::mem::size_of::<WINDOWPLACEMENT>() as u32;
    if GetWindowPlacement(hwnd, &mut wp) != 0 {
        let is_maximized = wp.showCmd
            == windows_sys::Win32::UI::WindowsAndMessaging::SW_SHOWMAXIMIZED as u32
            || (wp.flags & 0x0002) != 0; // WPF_RESTORETOMAXIMIZED

        let show_cmd = if is_maximized {
            windows_sys::Win32::UI::WindowsAndMessaging::SW_SHOWMAXIMIZED as u32
        } else {
            windows_sys::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL as u32
        };

        let rect =
            if wp.showCmd == windows_sys::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL as u32 {
                let mut r: RECT = std::mem::zeroed();
                if GetWindowRect(hwnd, &mut r) != 0 && r.right > r.left {
                    r
                } else {
                    wp.rcNormalPosition
                }
            } else {
                wp.rcNormalPosition
            };

        (
            show_cmd,
            WindowRect {
                left: rect.left,
                top: rect.top,
                right: rect.right,
                bottom: rect.bottom,
            },
        )
    } else {
        (
            windows_sys::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL as u32,
            WindowRect::default(),
        )
    }
}

/// Place `hwnd` according to `rule`.
///
/// `target_hmon` overrides which monitor the rule is interpreted against. The
/// default (`None`) infers it from the rect's centre, which is correct for
/// tray/IPC restores but wrong after a topology change — stale coordinates then
/// resolve to whichever monitor happens to cover them. Callers that know the
/// intended monitor by stable id pass it explicitly.
pub unsafe fn apply_rule_to_window(
    hwnd: HWND,
    rule: &WorkspaceRule,
    target_hmon: Option<HMONITOR>,
) {
    if rule.show_cmd == windows_sys::Win32::UI::WindowsAndMessaging::SW_SHOWMAXIMIZED as u32 {
        let mut wp: WINDOWPLACEMENT = std::mem::zeroed();
        wp.length = std::mem::size_of::<WINDOWPLACEMENT>() as u32;
        wp.showCmd = windows_sys::Win32::UI::WindowsAndMessaging::SW_SHOWMAXIMIZED as u32;
        // The normal-position rect decides which monitor the window maximizes
        // onto and where it lands when un-maximized; leaving it zeroed sends
        // the window to the primary display and collapses it on restore.
        //
        // It also has to agree with `target_hmon`. Chromium and Electron apps
        // keep a degenerate restore-down rect anchored at (0,0) — observed as
        // 647x154 and 750x155 — so a maximized window whose intended monitor is
        // the secondary would otherwise always maximize onto the primary.
        let mut normal = rule.rect.clone();
        if let Some(hmon) = target_hmon {
            let mut mi: MONITORINFO = std::mem::zeroed();
            mi.cbSize = std::mem::size_of::<MONITORINFO>() as u32;
            if GetMonitorInfoW(hmon, &mut mi) != 0 {
                normal = normal_pos_on_monitor(&normal, &mi.rcMonitor, &mi.rcWork);
            }
        }
        wp.rcNormalPosition = RECT {
            left: normal.left,
            top: normal.top,
            right: normal.right,
            bottom: normal.bottom,
        };
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
fn normal_pos_on_monitor(rect: &WindowRect, monitor: &RECT, work: &RECT) -> WindowRect {
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
fn detect_snap_halves(
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

pub unsafe fn match_rule_for_window(hwnd: HWND, rules: &[WorkspaceRule]) -> Option<WorkspaceRule> {
    if rules.is_empty() || !is_valid_window(hwnd) {
        return None;
    }

    let aumid = get_window_aumid(hwnd).to_lowercase();
    let exe_path = get_process_image_path(hwnd).to_lowercase();
    let class_name = get_window_class(hwnd).to_lowercase();
    let title = get_window_title(hwnd).to_lowercase();

    let mut best_rule: Option<WorkspaceRule> = None;
    let mut best_score = 0;

    for rule in rules {
        if let Some(score) = score_rule(&aumid, &exe_path, &class_name, &title, rule) {
            if score > best_score {
                best_score = score;
                best_rule = Some(rule.clone());
            }
        }
    }

    best_rule
}

/// Score how specifically `rule` identifies a window with the given
/// (lowercased) attributes. Returns `None` when any matcher the rule
/// specifies disagrees with the window.
pub fn score_rule(
    aumid: &str,
    exe_path: &str,
    class_name: &str,
    title: &str,
    rule: &WorkspaceRule,
) -> Option<i32> {
    let rule_aumid = rule.aumid.to_lowercase();
    let rule_exe = rule.exe_path.to_lowercase();
    let rule_class = rule.class_name.to_lowercase();
    let rule_title = rule.title_pattern.to_lowercase();

    let exe_name = std::path::Path::new(exe_path)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("");
    let rule_exe_name = std::path::Path::new(&rule_exe)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("")
        .to_string();

    let mut score = 0;

    // 1. AUMID Match (Highest specificity: 100 points!). Exact equality only:
    // AUMIDs are identifiers, and Chromium browsers use "Brave" for the
    // default profile and "Brave.<profile>" for others — a substring match
    // makes the default-profile rule swallow every profile's windows.
    if !rule_aumid.is_empty() {
        if !aumid.is_empty() && aumid == rule_aumid {
            score += 100;
        } else {
            return None;
        }
    }

    // 2. Executable Match (20 points)
    if !rule_exe.is_empty() {
        let matches_exe = exe_path == rule_exe
            || exe_path.ends_with(&rule_exe)
            || (!exe_name.is_empty() && exe_name == rule_exe_name);
        if matches_exe {
            score += 20;
        } else {
            return None;
        }
    }

    // 3. Class Match (10 points)
    if !rule_class.is_empty() {
        if class_name == rule_class {
            score += 10;
        } else {
            return None;
        }
    }

    // 4. Title Pattern Match (30 points). One direction only: matching
    // "rule contains window title" lets a window with a short transient
    // title (e.g. "w") match nearly any rule.
    if !rule_title.is_empty() {
        if title.contains(&rule_title) {
            score += 30;
        } else {
            return None;
        }
    }

    Some(score)
}

struct EnumState<'a> {
    mgr: &'a DesktopManager,
    rules: Vec<WorkspaceRule>,
}

unsafe extern "system" fn enum_windows_callback(hwnd: HWND, lparam: isize) -> i32 {
    let state = &mut *(lparam as *mut EnumState);
    if is_valid_window(hwnd) {
        if let Some((mon_idx, desk_idx)) = state.mgr.find_window(hwnd) {
            let aumid = get_window_aumid(hwnd);
            let exe_path = get_process_image_path(hwnd);
            let class_name = get_window_class(hwnd);
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

            state.rules.push(WorkspaceRule {
                name,
                aumid,
                exe_path,
                class_name,
                title_pattern,
                display_index: mon_idx,
                desktop_index: desk_idx,
                show_cmd,
                rect,
                is_snapped,
            });
        }
    }
    1
}

pub unsafe fn capture_active_workspace(mgr: &DesktopManager) -> Vec<WorkspaceRule> {
    let mut state = EnumState {
        mgr,
        rules: Vec::new(),
    };
    EnumWindows(Some(enum_windows_callback), &mut state as *mut _ as isize);
    drop_redundant_title_patterns(&mut state.rules);
    state.rules
}

/// Window titles are volatile (page navigation, unread counters, open file),
/// and a rule with a `title_pattern` is disqualified when the title no longer
/// matches — breaking restore for the common case. Keep the captured title as
/// a matcher only when several rules share the same app identity (AUMID + exe)
/// and the title is the only way to tell the windows apart.
fn drop_redundant_title_patterns(rules: &mut [WorkspaceRule]) {
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

/// Write every top-level window's metrics to `out_file`.
///
/// Strictly read-only, and it must stay that way: this runs as a separate
/// process while a daemon may be live. It deliberately takes no
/// `DesktopManager` — constructing one calls `reclaim_orphaned_windows`, which
/// un-cloaks every window the running daemon has hidden on inactive spaces.
/// The parameter used to exist and was never read; the constructor grew that
/// side effect later, silently making a diagnostic destructive.
pub unsafe fn dump_all_window_metrics(out_file: &str) {
    let mut output = format!(
        "=== WINSPACES RUST WINDOW DUMP ({}) ===\n",
        chrono_format_now()
    );
    let mut hwnd = windows_sys::Win32::UI::WindowsAndMessaging::GetWindow(
        windows_sys::Win32::UI::WindowsAndMessaging::GetDesktopWindow(),
        windows_sys::Win32::UI::WindowsAndMessaging::GW_CHILD,
    );
    while !hwnd.is_null() {
        let title = get_window_title(hwnd);
        let class_name = get_window_class(hwnd);
        let exe_path = get_process_image_path(hwnd);

        if !title.is_empty() || !class_name.is_empty() {
            let mut win_rect: RECT = std::mem::zeroed();
            GetWindowRect(hwnd, &mut win_rect);

            let mut frame_rect: RECT = std::mem::zeroed();
            windows_sys::Win32::Graphics::Dwm::DwmGetWindowAttribute(
                hwnd,
                windows_sys::Win32::Graphics::Dwm::DWMWA_EXTENDED_FRAME_BOUNDS as _,
                &mut frame_rect as *mut _ as _,
                std::mem::size_of::<RECT>() as u32,
            );

            let vis = IsWindowVisible(hwnd);
            let (show_cmd, rect) = get_window_placement_info(hwnd);

            let ex_style = windows_sys::Win32::UI::WindowsAndMessaging::GetWindowLongW(
                hwnd,
                windows_sys::Win32::UI::WindowsAndMessaging::GWL_EXSTYLE,
            ) as u32;
            let owner = windows_sys::Win32::UI::WindowsAndMessaging::GetAncestor(
                hwnd,
                windows_sys::Win32::UI::WindowsAndMessaging::GA_ROOTOWNER,
            );
            let mut cloaked: u32 = 0;
            windows_sys::Win32::Graphics::Dwm::DwmGetWindowAttribute(
                hwnd,
                windows_sys::Win32::Graphics::Dwm::DWMWA_CLOAKED as _,
                &mut cloaked as *mut _ as _,
                std::mem::size_of::<u32>() as u32,
            );
            let valid = is_valid_window(hwnd);

            let w = win_rect.right - win_rect.left;
            let h = win_rect.bottom - win_rect.top;

            let line = format!(
                "==========================================\nHWND: {:?}, Vis={}\nTitle: '{}'\nClass: '{}'\nExe: '{}'\nEligibility: Valid={}, ExStyle=0x{:08X}, RootOwner={:?}, Cloaked={}\nGetWindowRect: Left={}, Top={}, Right={}, Bottom={} [W={}, H={}]\nDwmFrameBounds: Left={}, Top={}, Right={}, Bottom={}\nPlacement: showCmd={}, NormalPos: Left={}, Top={}, Right={}, Bottom={}\n",
                hwnd, vis, title, class_name, exe_path,
                valid, ex_style, owner, cloaked,
                win_rect.left, win_rect.top, win_rect.right, win_rect.bottom, w, h,
                frame_rect.left, frame_rect.top, frame_rect.right, frame_rect.bottom,
                show_cmd, rect.left, rect.top, rect.right, rect.bottom
            );
            output.push_str(&line);
        }
        hwnd = windows_sys::Win32::UI::WindowsAndMessaging::GetWindow(
            hwnd,
            windows_sys::Win32::UI::WindowsAndMessaging::GW_HWNDNEXT,
        );
    }
    let _ = std::fs::write(out_file, output);
}

fn chrono_format_now() -> String {
    format!("{:?}", std::time::SystemTime::now())
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
    fn aumid_matching_is_exact_across_browser_profiles() {
        // Chromium: default profile AUMID is "Brave", other profiles are
        // "Brave.<profile>". The Personal rule must not claim Work windows.
        let personal = WorkspaceRule {
            name: "Brave Personal".into(),
            aumid: "Brave".into(),
            exe_path: r"C:\brave\brave.exe".into(),
            display_index: 0,
            desktop_index: 0,
            ..Default::default()
        };
        let work = WorkspaceRule {
            name: "Brave Work".into(),
            aumid: "Brave.Profile1".into(),
            exe_path: r"C:\brave\brave.exe".into(),
            display_index: 0,
            desktop_index: 3,
            ..Default::default()
        };

        let exe = r"c:\brave\brave.exe";
        // Work-profile window: only the Work rule may match.
        assert_eq!(score_rule("brave.profile1", exe, "", "", &personal), None);
        assert!(score_rule("brave.profile1", exe, "", "", &work).is_some());
        // Personal-profile window: only the Personal rule may match.
        assert!(score_rule("brave", exe, "", "", &personal).is_some());
        assert_eq!(score_rule("brave", exe, "", "", &work), None);
    }

    #[test]
    fn rule_with_aumid_rejects_window_without_one() {
        let rule = WorkspaceRule {
            aumid: "Brave".into(),
            ..Default::default()
        };
        assert_eq!(score_rule("", "", "", "", &rule), None);
    }

    #[test]
    fn title_pattern_matches_one_direction_only() {
        let rule = WorkspaceRule {
            exe_path: r"C:\apps\chrome.exe".into(),
            title_pattern: "gmail".into(),
            ..Default::default()
        };
        let exe = r"c:\apps\chrome.exe";
        assert!(score_rule("", exe, "", "gmail - google chrome", &rule).is_some());
        // Window title being a substring of the pattern must NOT match.
        assert_eq!(score_rule("", exe, "", "g", &rule), None);
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
