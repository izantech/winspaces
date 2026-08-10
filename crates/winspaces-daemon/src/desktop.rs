use crate::log_info;
use std::ffi::c_void;
use std::ptr::{null, null_mut};
use windows_sys::Win32::Foundation::{BOOL, HWND, LPARAM, POINT, RECT};
use windows_sys::Win32::Graphics::Dwm::{
    DwmGetWindowAttribute, DwmSetWindowAttribute, DWMWA_CLOAK, DWMWA_CLOAKED,
};
use windows_sys::Win32::Graphics::Gdi::{
    EnumDisplayMonitors, GetMonitorInfoW, MonitorFromPoint, MonitorFromWindow, HDC, HMONITOR,
    MONITORINFOEXW, MONITOR_DEFAULTTONEAREST,
};
use windows_sys::Win32::System::SystemInformation::GetTickCount;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::SetActiveWindow;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GetAncestor, GetClassNameW, GetCursorPos, GetForegroundWindow, GetPropA,
    GetWindowLongW, GetWindowTextW, GetWindowThreadProcessId, IsIconic, IsWindow, RemovePropA,
    SetForegroundWindow, SetPropA, SetWindowPos, ShowWindow, GA_ROOTOWNER, GWL_EXSTYLE, GWL_STYLE,
    SWP_FRAMECHANGED, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SWP_NOZORDER, SWP_SHOWWINDOW,
    SW_FORCEMINIMIZE, SW_HIDE, SW_SHOW, SW_SHOWMINNOACTIVE, SW_SHOWNOACTIVATE, WS_EX_APPWINDOW,
    WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_VISIBLE,
};
use winspaces_common::NUM_DESKTOPS;

pub const MAX_MONITORS: usize = 8;
const WINSPACES_PROP_STATE: &[u8] = b"WinSpacesWindowState\0";

const WINSPACES_STATE_TRACKED: usize = 0x01;
const WINSPACES_STATE_WAS_ICONIC: usize = 0x02;
const WINSPACES_STATE_FORCED_MINIMIZED: usize = 0x04;
const WINSPACES_STATE_CLOAKED: usize = 0x08;
// System windows (input experience, task host, ...) that we cloaked out of the
// way. Marked so exit/startup passes can undo the cloak: DWM cloaks persist
// after the process that applied them dies.
const WINSPACES_STATE_SYSTEM_HIDDEN: usize = 0x10;

/// Facts about a window that the eligibility decision needs, gathered from
/// Win32 by `is_valid_window` so the decision itself (`is_eligible`) stays
/// pure and unit-testable.
struct WindowFacts {
    style: u32,
    ex_style: u32,
    class_name: String,
    has_title: bool,
    /// DWM-cloaked without our CLOAKED state bit: cloaked by Windows or
    /// another app (suspended UWP, native virtual desktops, ...).
    externally_cloaked: bool,
    /// Our state bits say we hid it (cloak or forced minimize), so a clear
    /// WS_VISIBLE / set cloak must not disqualify it.
    hidden_by_us: bool,
    /// Root-owner facts; None when unowned or when WS_EX_APPWINDOW asks to
    /// be judged standalone.
    owner: Option<Box<WindowFacts>>,
}

/// Shell/system window classes that are never managed. Classes are stable
/// identifiers across locales and Windows builds, unlike window titles.
fn is_shell_class(class_name: &str) -> bool {
    matches!(
        class_name,
        "Progman"
            | "WorkerW"
            | "Shell_TrayWnd"
            | "Shell_SecondaryTrayWnd"
            | "Windows.UI.Core.CoreWindow"
            | "EdgeUiInputTopWndClass"
            | "XamlExplorerHost"
            | "TopLevelWindowForOverflowXamlIsland"
            | "Windows.UI.Composition.DesktopWindowTarget"
            | "TaskListThumbnailWnd"
            | "NativeHWNDHost"
            | "PopupHost"
            | "Xaml_WindowedPopupClass"
            | "IME"
            | "MSCTFIME UI"
            | "tooltips_class32"
            | "SysShadow"
            | "ComboLBox"
            | "#32768"
    )
}

/// Structural noise checks shared by a window and its root owner, modeled on
/// the shell's Alt-Tab rules (visibility, extended styles, cloak state)
/// instead of title blacklists. `require_title` is off for the owner: only
/// the window itself must be titled.
fn passes_structural_checks(facts: &WindowFacts, require_title: bool) -> bool {
    if (facts.ex_style & WS_EX_TOOLWINDOW) != 0 {
        return false;
    }
    // Never-activated windows (overlays, OSDs) don't appear on the taskbar
    // unless WS_EX_APPWINDOW forces them in.
    if (facts.ex_style & WS_EX_NOACTIVATE) != 0 && (facts.ex_style & WS_EX_APPWINDOW) == 0 {
        return false;
    }
    if is_shell_class(&facts.class_name) {
        return false;
    }
    if require_title && !facts.has_title {
        return false;
    }
    if facts.externally_cloaked {
        return false;
    }
    if (facts.style & WS_VISIBLE) == 0 && !facts.hidden_by_us {
        return false;
    }
    true
}

/// A window is manageable when it passes the structural checks itself and,
/// if owned, its root owner does too. Owned dialogs of real apps stay
/// individually managed (they must cloak with their app on space switches);
/// popups of hidden or tool-window owners are noise.
fn is_eligible(facts: &WindowFacts) -> bool {
    if !passes_structural_checks(facts, true) {
        return false;
    }
    match &facts.owner {
        Some(owner) => passes_structural_checks(owner, false),
        None => true,
    }
}

unsafe fn gather_window_facts(hwnd: HWND, follow_owner: bool) -> WindowFacts {
    let style = GetWindowLongW(hwnd, GWL_STYLE) as u32;
    let ex_style = GetWindowLongW(hwnd, GWL_EXSTYLE) as u32;

    let mut class_buf = [0u16; 256];
    let clen = GetClassNameW(hwnd, class_buf.as_mut_ptr(), class_buf.len() as i32);
    let class_name = if clen > 0 {
        String::from_utf16_lossy(&class_buf[..clen as usize])
    } else {
        String::new()
    };

    let mut title = [0u16; 2];
    let has_title = GetWindowTextW(hwnd, title.as_mut_ptr(), title.len() as i32) > 0;

    let state = get_window_state(hwnd);
    let mut cloaked: u32 = 0;
    let hr = DwmGetWindowAttribute(
        hwnd,
        DWMWA_CLOAKED as _,
        &mut cloaked as *mut _ as _,
        std::mem::size_of::<u32>() as u32,
    );
    let externally_cloaked = hr == 0 && cloaked != 0 && (state & WINSPACES_STATE_CLOAKED) == 0;
    let hidden_by_us = (state & (WINSPACES_STATE_CLOAKED | WINSPACES_STATE_FORCED_MINIMIZED)) != 0;

    let owner = if follow_owner && (ex_style & WS_EX_APPWINDOW) == 0 {
        let root = GetAncestor(hwnd, GA_ROOTOWNER);
        if !root.is_null() && root != hwnd {
            Some(Box::new(gather_window_facts(root, false)))
        } else {
            None
        }
    } else {
        None
    };

    WindowFacts {
        style,
        ex_style,
        class_name,
        has_title,
        externally_cloaked,
        hidden_by_us,
        owner,
    }
}

pub fn is_valid_window(hwnd: HWND) -> bool {
    unsafe {
        if hwnd.is_null() || IsWindow(hwnd) == 0 {
            return false;
        }

        let mut pid: u32 = 0;
        GetWindowThreadProcessId(hwnd, &mut pid);
        if pid == 0 || pid == std::process::id() {
            return false;
        }

        is_eligible(&gather_window_facts(hwnd, true))
    }
}

fn get_window_state(hwnd: HWND) -> usize {
    unsafe { GetPropA(hwnd, WINSPACES_PROP_STATE.as_ptr()) as usize }
}

fn set_window_state(hwnd: HWND, state: usize) {
    unsafe {
        if state == 0 {
            RemovePropA(hwnd, WINSPACES_PROP_STATE.as_ptr());
        } else {
            SetPropA(hwnd, WINSPACES_PROP_STATE.as_ptr(), state as *mut c_void);
        }
    }
}

pub struct MonitorState {
    pub hmon: HMONITOR,
    /// Stable device name (`\\.\DISPLAY1`, ...) used to re-associate state
    /// across WM_DISPLAYCHANGE, where HMONITOR handles may be reissued.
    pub device: [u16; 32],
    pub current: usize,
    pub last_switched_desk: usize,
    pub last_switch_time: u32,
    pub suppress_foreground_until: u32,
    pub desktops: [Vec<HWND>; NUM_DESKTOPS],
}

fn monitor_device_name(hmon: HMONITOR) -> [u16; 32] {
    unsafe {
        let mut mi: MONITORINFOEXW = std::mem::zeroed();
        mi.monitorInfo.cbSize = std::mem::size_of::<MONITORINFOEXW>() as u32;
        if GetMonitorInfoW(hmon, &mut mi as *mut _ as *mut _) != 0 {
            mi.szDevice
        } else {
            [0u16; 32]
        }
    }
}

impl MonitorState {
    pub fn new(hmon: HMONITOR) -> Self {
        const EMPTY_VEC: Vec<HWND> = Vec::new();
        Self {
            hmon,
            device: monitor_device_name(hmon),
            current: 0,
            last_switched_desk: 0,
            last_switch_time: 0,
            suppress_foreground_until: 0,
            desktops: [EMPTY_VEC; NUM_DESKTOPS],
        }
    }

    #[allow(dead_code)]
    pub fn is_window_on_monitor(&self, hwnd: HWND) -> bool {
        unsafe {
            let hmon = MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST);
            hmon == self.hmon
        }
    }
}

pub struct DesktopManager {
    pub monitors: Vec<MonitorState>,
    pub handle_hotkeys: bool,
    pub show_all_taskbar: bool,
    pub suppress_foreground: bool,
}

impl DesktopManager {
    pub fn new() -> Self {
        // A previous instance may have died (crash, taskkill) leaving windows
        // cloaked/minimized with our state props still attached. Restore them
        // before scanning, otherwise they stay invisible forever.
        reclaim_orphaned_windows();
        let mut mgr = Self {
            monitors: Vec::new(),
            handle_hotkeys: true,
            show_all_taskbar: true,
            suppress_foreground: false,
        };
        mgr.update_monitors();
        mgr.scan_untracked_windows();
        mgr
    }

    pub fn set_show_all_taskbar(&mut self, new_val: bool) {
        if self.show_all_taskbar == new_val {
            return;
        }
        log_info!(
            "Changing show_all_taskbar mode from {} to {}",
            self.show_all_taskbar,
            new_val
        );

        let old_val = self.show_all_taskbar;
        self.show_all_taskbar = new_val;

        // Re-hide windows on inactive spaces in the new mode: show with the old
        // mode first so the hide path doesn't early-return on the already-set
        // CLOAKED/FORCED_MINIMIZED state bits.
        for m_idx in 0..self.monitors.len() {
            let current = self.monitors[m_idx].current;
            for d_idx in 0..NUM_DESKTOPS {
                if d_idx == current {
                    continue;
                }
                let windows = self.monitors[m_idx].desktops[d_idx].clone();
                for &hwnd in &windows {
                    if is_valid_window(hwnd) {
                        set_window_visibility(hwnd, true, old_val);
                        set_window_visibility(hwnd, false, new_val);
                    }
                }
            }
        }
    }

    pub fn update_monitors(&mut self) {
        self.monitors.clear();
        unsafe {
            EnumDisplayMonitors(
                null_mut(),
                null(),
                Some(enum_monitors_callback),
                &mut self.monitors as *mut _ as LPARAM,
            );
        }
    }

    /// Rebuild the monitor list after WM_DISPLAYCHANGE, carrying per-monitor
    /// space state over by device name (HMONITORs may be reissued). Windows
    /// tracked on a vanished monitor are made visible and re-scanned onto
    /// whichever monitor the OS moved them to.
    pub fn handle_display_change(&mut self) {
        let old_monitors = std::mem::take(&mut self.monitors);
        self.update_monitors();
        log_info!(
            "Display change: {} -> {} monitors",
            old_monitors.len(),
            self.monitors.len()
        );

        let show_all = self.show_all_taskbar;
        for old in old_monitors {
            let new_idx = self
                .monitors
                .iter()
                .position(|m| m.device == old.device && old.device[0] != 0);
            match new_idx {
                Some(idx) => {
                    self.monitors[idx].current = old.current;
                    self.monitors[idx].last_switched_desk = old.last_switched_desk;
                    self.monitors[idx].last_switch_time = old.last_switch_time;
                    self.monitors[idx].desktops = old.desktops;
                }
                None => {
                    for desk in &old.desktops {
                        for &hwnd in desk {
                            if is_valid_window(hwnd) {
                                set_window_visibility(hwnd, true, show_all);
                            }
                            set_window_state(hwnd, 0);
                        }
                    }
                }
            }
        }
        self.scan_untracked_windows();
    }

    pub fn get_active_monitor_index(&self) -> usize {
        if self.monitors.is_empty() {
            return 0;
        }
        unsafe {
            let mut pt = POINT { x: 0, y: 0 };
            GetCursorPos(&mut pt);
            let hmon = MonitorFromPoint(pt, MONITOR_DEFAULTTONEAREST);
            for (idx, mon) in self.monitors.iter().enumerate() {
                if mon.hmon == hmon {
                    return idx;
                }
            }

            let fg = GetForegroundWindow();
            if !fg.is_null() {
                let hmon_fg = MonitorFromWindow(fg, MONITOR_DEFAULTTONEAREST);
                for (idx, mon) in self.monitors.iter().enumerate() {
                    if mon.hmon == hmon_fg {
                        return idx;
                    }
                }
            }
        }
        0
    }

    pub fn find_window(&self, hwnd: HWND) -> Option<(usize, usize)> {
        for (m_idx, mon) in self.monitors.iter().enumerate() {
            for (d_idx, desk) in mon.desktops.iter().enumerate() {
                if desk.contains(&hwnd) {
                    return Some((m_idx, d_idx));
                }
            }
        }
        None
    }

    fn remove_window(&mut self, hwnd: HWND) -> bool {
        let mut removed = false;
        for mon in &mut self.monitors {
            for desk in &mut mon.desktops {
                if let Some(pos) = desk.iter().position(|&h| h == hwnd) {
                    desk.remove(pos);
                    removed = true;
                }
            }
        }
        if removed {
            set_window_state(hwnd, 0);
        }
        removed
    }

    pub fn track_window(&mut self, hwnd: HWND, mon_idx: usize, desk_idx: usize) {
        self.remove_window(hwnd);

        if is_valid_window(hwnd) {
            let mut state = WINSPACES_STATE_TRACKED;
            unsafe {
                if IsIconic(hwnd) != 0 {
                    state |= WINSPACES_STATE_WAS_ICONIC;
                }
            }
            set_window_state(hwnd, state);
            self.monitors[mon_idx].desktops[desk_idx].push(hwnd);
            log_info!(
                "track_window: hwnd {:?} -> Mon {}, Desk {}",
                hwnd,
                mon_idx + 1,
                desk_idx + 1
            );
        }
    }

    pub fn scan_untracked_windows(&mut self) {
        struct ScanContext<'a> {
            mgr: &'a mut DesktopManager,
        }

        unsafe extern "system" fn enum_windows_proc(hwnd: HWND, lparam: LPARAM) -> BOOL {
            let ctx = &mut *(lparam as *mut ScanContext);

            let mut title = [0u16; 256];
            let len = GetWindowTextW(hwnd, title.as_mut_ptr(), title.len() as i32);
            if len > 0 {
                let title_str = String::from_utf16_lossy(&title[..len as usize]);
                let lower_title = title_str.to_lowercase();
                // Exact titles only: a substring match here would cloak
                // legitimate user windows (e.g. a browser tab mentioning
                // "input experience").
                if lower_title == "windows input experience"
                    || lower_title == "experiencia de entrada de windows"
                    || lower_title == "task host window"
                    || lower_title == "windows push notifications platform"
                    || lower_title == "coremessaging"
                    || lower_title == "default ime"
                    || lower_title == "msctfime ui"
                    || lower_title == "popuphost"
                {
                    let state = get_window_state(hwnd);
                    if (state & WINSPACES_STATE_SYSTEM_HIDDEN) == 0 {
                        let mut one: i32 = 1;
                        DwmSetWindowAttribute(
                            hwnd,
                            DWMWA_CLOAK as _,
                            &mut one as *mut _ as _,
                            std::mem::size_of::<i32>() as u32,
                        );
                        ShowWindow(hwnd, SW_HIDE);
                        // Mark it so exit/startup passes can undo the cloak.
                        set_window_state(hwnd, state | WINSPACES_STATE_SYSTEM_HIDDEN);
                    }
                    return 1;
                }
            }

            let state = get_window_state(hwnd);
            if (state & (WINSPACES_STATE_CLOAKED | WINSPACES_STATE_FORCED_MINIMIZED)) != 0 {
                return 1;
            }

            if is_valid_window(hwnd) {
                let hmon = MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST);
                let actual_mon_idx = ctx
                    .mgr
                    .monitors
                    .iter()
                    .position(|m| m.hmon == hmon)
                    .unwrap_or(0);

                match ctx.mgr.find_window(hwnd) {
                    None => {
                        let desk_idx = ctx.mgr.monitors[actual_mon_idx].current;
                        ctx.mgr.track_window(hwnd, actual_mon_idx, desk_idx);
                    }
                    Some((curr_mon, _curr_desk)) if curr_mon != actual_mon_idx => {
                        let desk_idx = ctx.mgr.monitors[actual_mon_idx].current;
                        log_info!(
                            "Window {:?} moved across displays from Mon {} to Mon {} (Space {})",
                            hwnd,
                            curr_mon + 1,
                            actual_mon_idx + 1,
                            desk_idx + 1
                        );
                        ctx.mgr.track_window(hwnd, actual_mon_idx, desk_idx);
                    }
                    _ => {}
                }
            }
            1
        }

        let mut ctx = ScanContext { mgr: self };
        unsafe {
            EnumWindows(Some(enum_windows_proc), &mut ctx as *mut _ as LPARAM);
        }
    }

    pub fn go_to_desk(&mut self, target_desk: usize) {
        if target_desk >= NUM_DESKTOPS {
            return;
        }
        let mon_idx = self.get_active_monitor_index();
        if self.monitors.is_empty() {
            return;
        }

        if self.monitors[mon_idx].current == target_desk {
            return;
        }

        self.switch_desktop(mon_idx, target_desk, None);
    }

    pub fn step_desktop(&mut self, delta: i32) {
        if self.monitors.is_empty() {
            return;
        }
        let mon_idx = self.get_active_monitor_index();
        let cur = self.monitors[mon_idx].current as i32;
        let num = NUM_DESKTOPS as i32;
        let next = (cur + delta).rem_euclid(num) as usize;
        self.go_to_desk(next);
    }

    pub fn move_to_desk(&mut self, target_desk: usize) {
        if target_desk >= NUM_DESKTOPS {
            return;
        }
        let fg = unsafe { GetForegroundWindow() };
        if fg.is_null() || !is_valid_window(fg) {
            return;
        }

        let mon_idx = self.get_active_monitor_index();
        let cur = self.monitors[mon_idx].current;
        if cur == target_desk {
            return;
        }

        self.track_window(fg, mon_idx, target_desk);
        self.switch_desktop(mon_idx, target_desk, Some(fg));
    }

    pub fn step_move_window(&mut self, delta: i32) {
        if self.monitors.is_empty() {
            return;
        }
        let mon_idx = self.get_active_monitor_index();
        let cur = self.monitors[mon_idx].current as i32;
        let num = NUM_DESKTOPS as i32;
        let next = (cur + delta).rem_euclid(num) as usize;
        self.move_to_desk(next);
    }

    pub fn switch_desktop(
        &mut self,
        mon_idx: usize,
        target_desk: usize,
        activate_window: Option<HWND>,
    ) {
        if mon_idx >= self.monitors.len() || target_desk >= NUM_DESKTOPS {
            return;
        }

        self.scan_untracked_windows();

        let old_desk = self.monitors[mon_idx].current;
        let now = unsafe { GetTickCount() };

        log_info!(
            "switch_desktop: Mon {} from Space {} to Space {}",
            mon_idx + 1,
            old_desk + 1,
            target_desk + 1
        );

        self.monitors[mon_idx].last_switched_desk = old_desk;
        self.monitors[mon_idx].last_switch_time = now;
        self.monitors[mon_idx].suppress_foreground_until = now.wrapping_add(500);
        self.monitors[mon_idx].current = target_desk;

        let show_all = self.show_all_taskbar;

        for d_idx in 0..NUM_DESKTOPS {
            let windows = self.monitors[mon_idx].desktops[d_idx].clone();
            if d_idx == target_desk {
                continue;
            }
            for &hwnd in &windows {
                if is_valid_window(hwnd) {
                    set_window_visibility(hwnd, false, show_all);
                }
            }
        }

        let target_windows = self.monitors[mon_idx].desktops[target_desk].clone();
        for &hwnd in &target_windows {
            if is_valid_window(hwnd) {
                set_window_visibility(hwnd, true, show_all);
            }
        }

        if let Some(act_hwnd) = activate_window {
            unsafe {
                SetForegroundWindow(act_hwnd);
                SetActiveWindow(act_hwnd);
            }
        } else {
            let mut activated = false;
            for &hwnd in target_windows.iter().rev() {
                if is_valid_window(hwnd) {
                    let state = get_window_state(hwnd);
                    if (state & WINSPACES_STATE_WAS_ICONIC) == 0 {
                        unsafe {
                            SetForegroundWindow(hwnd);
                        }
                        activated = true;
                        break;
                    }
                }
            }

            if !activated {
                if let Some(&first_hwnd) = target_windows.iter().next_back() {
                    if is_valid_window(first_hwnd) {
                        unsafe {
                            SetForegroundWindow(first_hwnd);
                        }
                    }
                }
            }
        }
    }

    pub fn windows_show_all(&mut self) {
        log_info!("Restoring visibility for all managed windows");
        let show_all = self.show_all_taskbar;
        for mon in &mut self.monitors {
            for desk in &mut mon.desktops {
                for &hwnd in desk.iter() {
                    if is_valid_window(hwnd) {
                        set_window_visibility(hwnd, true, show_all);
                        set_window_state(hwnd, 0);
                    }
                }
                desk.clear();
            }
        }
        // Catch anything the tracked lists missed: system windows we cloaked
        // and windows whose validity changed since tracking.
        reclaim_orphaned_windows();
    }
}

/// Restore every top-level window still carrying a WinSpaces state prop and
/// clear the prop. Runs at startup (recovers windows stranded by a crashed
/// instance — DWM cloaks and props outlive the process) and on clean exit.
pub fn reclaim_orphaned_windows() {
    unsafe extern "system" fn enum_proc(hwnd: HWND, _lparam: LPARAM) -> BOOL {
        let state = get_window_state(hwnd);
        if state == 0 {
            return 1;
        }
        if (state & WINSPACES_STATE_SYSTEM_HIDDEN) != 0 {
            let mut zero: i32 = 0;
            DwmSetWindowAttribute(
                hwnd,
                DWMWA_CLOAK as _,
                &mut zero as *mut _ as _,
                std::mem::size_of::<i32>() as u32,
            );
            ShowWindow(hwnd, SW_SHOWNOACTIVATE);
        } else if (state & (WINSPACES_STATE_CLOAKED | WINSPACES_STATE_FORCED_MINIMIZED)) != 0 {
            set_window_visibility(hwnd, true, false);
        }
        set_window_state(hwnd, 0);
        1
    }
    unsafe {
        EnumWindows(Some(enum_proc), 0);
    }
}

pub fn set_window_visibility(hwnd: HWND, visible: bool, show_all_taskbar: bool) {
    unsafe {
        let mut state = get_window_state(hwnd);
        if state == 0 {
            return;
        }

        if visible {
            let was_forced = (state & WINSPACES_STATE_FORCED_MINIMIZED) != 0;
            let was_cloaked = (state & WINSPACES_STATE_CLOAKED) != 0;

            if was_cloaked {
                let mut zero: i32 = 0;
                DwmSetWindowAttribute(
                    hwnd,
                    DWMWA_CLOAK as _,
                    &mut zero as *mut _ as _,
                    std::mem::size_of::<i32>() as u32,
                );
                state &= !WINSPACES_STATE_CLOAKED;
                SetWindowPos(
                    hwnd,
                    std::ptr::null_mut(),
                    0,
                    0,
                    0,
                    0,
                    SWP_NOMOVE
                        | SWP_NOSIZE
                        | SWP_NOZORDER
                        | SWP_NOACTIVATE
                        | SWP_FRAMECHANGED
                        | SWP_SHOWWINDOW,
                );
            }

            if (state & WINSPACES_STATE_WAS_ICONIC) != 0 {
                ShowWindow(hwnd, SW_SHOWMINNOACTIVE);
            } else if was_forced {
                let mut wp: windows_sys::Win32::UI::WindowsAndMessaging::WINDOWPLACEMENT =
                    std::mem::zeroed();
                wp.length = std::mem::size_of::<
                    windows_sys::Win32::UI::WindowsAndMessaging::WINDOWPLACEMENT,
                >() as u32;
                if windows_sys::Win32::UI::WindowsAndMessaging::GetWindowPlacement(hwnd, &mut wp)
                    != 0
                    && (wp.flags & 0x0002) != 0
                {
                    ShowWindow(
                        hwnd,
                        windows_sys::Win32::UI::WindowsAndMessaging::SW_SHOWMAXIMIZED,
                    );
                } else {
                    ShowWindow(
                        hwnd,
                        windows_sys::Win32::UI::WindowsAndMessaging::SW_RESTORE,
                    );
                }
                state &= !WINSPACES_STATE_FORCED_MINIMIZED;
            } else {
                ShowWindow(hwnd, SW_SHOW);
            }
        } else {
            if (state & (WINSPACES_STATE_CLOAKED | WINSPACES_STATE_FORCED_MINIMIZED)) != 0 {
                return;
            }

            if IsIconic(hwnd) != 0 {
                state |= WINSPACES_STATE_WAS_ICONIC;
            } else {
                state &= !WINSPACES_STATE_WAS_ICONIC;
            }

            if show_all_taskbar {
                if IsIconic(hwnd) == 0 {
                    ShowWindow(hwnd, SW_FORCEMINIMIZE);
                    state |= WINSPACES_STATE_FORCED_MINIMIZED;
                }
            } else {
                let mut one: i32 = 1;
                let hr = DwmSetWindowAttribute(
                    hwnd,
                    DWMWA_CLOAK as _,
                    &mut one as *mut _ as _,
                    std::mem::size_of::<i32>() as u32,
                );
                if hr >= 0 {
                    state |= WINSPACES_STATE_CLOAKED;
                } else {
                    ShowWindow(hwnd, SW_HIDE);
                }
            }
        }
        set_window_state(hwnd, state);
    }
}

unsafe extern "system" fn enum_monitors_callback(
    hmon: HMONITOR,
    _: HDC,
    _: *mut RECT,
    lparam: LPARAM,
) -> BOOL {
    let monitors = &mut *(lparam as *mut Vec<MonitorState>);
    if monitors.len() < MAX_MONITORS {
        monitors.push(MonitorState::new(hmon));
    }
    1
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A plain visible, titled, unowned app window.
    fn app_window() -> WindowFacts {
        WindowFacts {
            style: WS_VISIBLE,
            ex_style: 0,
            class_name: "Chrome_WidgetWin_1".to_string(),
            has_title: true,
            externally_cloaked: false,
            hidden_by_us: false,
            owner: None,
        }
    }

    #[test]
    fn plain_app_window_is_eligible() {
        assert!(is_eligible(&app_window()));
    }

    #[test]
    fn tool_window_is_excluded_even_with_appwindow() {
        let mut f = app_window();
        f.ex_style = WS_EX_TOOLWINDOW;
        assert!(!is_eligible(&f));
        f.ex_style = WS_EX_TOOLWINDOW | WS_EX_APPWINDOW;
        assert!(!is_eligible(&f));
    }

    #[test]
    fn noactivate_is_excluded_unless_appwindow() {
        let mut f = app_window();
        f.ex_style = WS_EX_NOACTIVATE;
        assert!(!is_eligible(&f));
        f.ex_style = WS_EX_NOACTIVATE | WS_EX_APPWINDOW;
        assert!(is_eligible(&f));
    }

    #[test]
    fn shell_classes_are_excluded() {
        for class in [
            "Progman",
            "Shell_TrayWnd",
            "Windows.UI.Core.CoreWindow",
            "IME",
        ] {
            let mut f = app_window();
            f.class_name = class.to_string();
            assert!(!is_eligible(&f), "{class} must be excluded");
        }
    }

    #[test]
    fn untitled_window_is_excluded_but_untitled_owner_is_fine() {
        let mut f = app_window();
        f.has_title = false;
        assert!(!is_eligible(&f));

        let mut owned = app_window();
        let mut owner = app_window();
        owner.has_title = false;
        owned.owner = Some(Box::new(owner));
        assert!(is_eligible(&owned));
    }

    #[test]
    fn dialog_with_eligible_owner_is_included() {
        let mut f = app_window();
        f.owner = Some(Box::new(app_window()));
        assert!(is_eligible(&f));
    }

    #[test]
    fn popup_with_invisible_or_tool_window_owner_is_excluded() {
        let mut hidden_owner = app_window();
        hidden_owner.style = 0;
        let mut f = app_window();
        f.owner = Some(Box::new(hidden_owner));
        assert!(!is_eligible(&f));

        let mut tool_owner = app_window();
        tool_owner.ex_style = WS_EX_TOOLWINDOW;
        f.owner = Some(Box::new(tool_owner));
        assert!(!is_eligible(&f));
    }

    #[test]
    fn dialog_of_owner_we_cloaked_stays_eligible() {
        // Space switched away while a dialog was up: the owner is cloaked by
        // us but the dialog must remain manageable so it cloaks too.
        let mut cloaked_owner = app_window();
        cloaked_owner.hidden_by_us = true;
        cloaked_owner.style = 0;
        let mut f = app_window();
        f.owner = Some(Box::new(cloaked_owner));
        assert!(is_eligible(&f));
    }

    #[test]
    fn own_cloak_state_bit_keeps_window_eligible() {
        let mut f = app_window();
        f.hidden_by_us = true;
        f.style = 0;
        assert!(is_eligible(&f));
    }

    #[test]
    fn externally_cloaked_window_is_excluded() {
        let mut f = app_window();
        f.externally_cloaked = true;
        assert!(!is_eligible(&f));
    }

    #[test]
    fn invisible_window_without_our_state_is_excluded() {
        let mut f = app_window();
        f.style = 0;
        assert!(!is_eligible(&f));
    }
}
