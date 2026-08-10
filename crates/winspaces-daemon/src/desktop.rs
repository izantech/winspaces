use crate::log_info;
use std::ffi::c_void;
use std::ptr::{null, null_mut};
use windows_sys::Win32::Foundation::{BOOL, HWND, LPARAM, POINT, RECT};
use windows_sys::Win32::Graphics::Dwm::{
    DwmGetWindowAttribute, DwmSetWindowAttribute, DWMWA_CLOAK, DWMWA_CLOAKED,
};
use windows_sys::Win32::Graphics::Gdi::{
    EnumDisplayMonitors, MonitorFromPoint, MonitorFromWindow, HDC, HMONITOR,
    MONITOR_DEFAULTTONEAREST,
};
use windows_sys::Win32::System::SystemInformation::GetTickCount;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::SetActiveWindow;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GetClassNameW, GetCursorPos, GetForegroundWindow, GetPropA, GetWindowLongW,
    GetWindowTextW, GetWindowThreadProcessId, IsIconic, IsWindow, RemovePropA, SetForegroundWindow,
    SetPropA, SetWindowPos, ShowWindow, GWL_EXSTYLE, GWL_STYLE, SWP_FRAMECHANGED, SWP_NOACTIVATE,
    SWP_NOMOVE, SWP_NOSIZE, SWP_NOZORDER, SWP_SHOWWINDOW, SW_FORCEMINIMIZE, SW_HIDE, SW_SHOW,
    SW_SHOWMINNOACTIVE, WS_EX_TOOLWINDOW, WS_VISIBLE,
};
use winspaces_common::NUM_DESKTOPS;

pub const MAX_MONITORS: usize = 8;
const WINSPACES_PROP_STATE: &[u8] = b"WinSpacesWindowState\0";

const WINSPACES_STATE_TRACKED: usize = 0x01;
const WINSPACES_STATE_WAS_ICONIC: usize = 0x02;
const WINSPACES_STATE_FORCED_MINIMIZED: usize = 0x04;
const WINSPACES_STATE_CLOAKED: usize = 0x08;

pub fn is_valid_window(hwnd: HWND) -> bool {
    unsafe {
        if IsWindow(hwnd) == 0 || hwnd.is_null() {
            return false;
        }

        let mut pid: u32 = 0;
        GetWindowThreadProcessId(hwnd, &mut pid);
        if pid == 0 || pid == std::process::id() {
            return false;
        }

        let ex_style = GetWindowLongW(hwnd, GWL_EXSTYLE) as u32;
        if (ex_style & WS_EX_TOOLWINDOW) != 0 {
            return false;
        }

        let mut class_buf = [0u16; 256];
        let clen = GetClassNameW(hwnd, class_buf.as_mut_ptr(), class_buf.len() as i32);
        let class_name = if clen > 0 {
            String::from_utf16_lossy(&class_buf[..clen as usize])
        } else {
            String::new()
        };

        if class_name == "Progman"
            || class_name == "WorkerW"
            || class_name == "Shell_TrayWnd"
            || class_name == "Shell_SecondaryTrayWnd"
            || class_name == "Windows.UI.Core.CoreWindow"
            || class_name == "EdgeUiInputTopWndClass"
            || class_name == "XamlExplorerHost"
            || class_name == "TopLevelWindowForOverflowXamlIsland"
            || class_name == "Windows.UI.Composition.DesktopWindowTarget"
            || class_name == "TaskListThumbnailWnd"
            || class_name == "NativeHWNDHost"
            || class_name == "PopupHost"
            || class_name == "tooltips_class32"
            || class_name == "SysShadow"
            || class_name == "ComboLBox"
            || class_name == "#32768"
        {
            return false;
        }

        let mut title = [0u16; 256];
        let len = GetWindowTextW(hwnd, title.as_mut_ptr(), title.len() as i32);
        if len == 0 {
            return false;
        }
        let title_str = String::from_utf16_lossy(&title[..len as usize]);
        let lower_title = title_str.to_lowercase();
        if lower_title.contains("windows input experience")
            || lower_title.contains("input experience")
            || lower_title.contains("experiencia de entrada")
            || lower_title.contains("task host window")
            || lower_title.contains("windows push notifications")
            || lower_title.contains("coremessaging")
            || lower_title == "program manager"
            || lower_title == "windows shell experience host"
            || lower_title == "popuphost"
            || lower_title.starts_with("popuphost")
            || lower_title == "default ime"
            || lower_title == "msctfime ui"
        {
            return false;
        }

        // Check cloaked state: Ignore background system services cloaked by Windows
        let state = get_window_state(hwnd);
        let mut cloaked: u32 = 0;
        let hr = DwmGetWindowAttribute(
            hwnd,
            DWMWA_CLOAKED as _,
            &mut cloaked as *mut _ as _,
            std::mem::size_of::<u32>() as u32,
        );
        if hr == 0 && cloaked != 0 && (state & WINSPACES_STATE_CLOAKED) == 0 {
            return false;
        }

        let style = GetWindowLongW(hwnd, GWL_STYLE) as u32;
        if (style & WS_VISIBLE) == 0
            && (state & (WINSPACES_STATE_CLOAKED | WINSPACES_STATE_FORCED_MINIMIZED)) == 0
        {
            return false;
        }

        true
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
    pub current: usize,
    pub last_switched_desk: usize,
    pub last_switch_time: u32,
    pub suppress_foreground_until: u32,
    pub desktops: [Vec<HWND>; NUM_DESKTOPS],
}

impl MonitorState {
    pub fn new(hmon: HMONITOR) -> Self {
        const EMPTY_VEC: Vec<HWND> = Vec::new();
        Self {
            hmon,
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

        for m_idx in 0..self.monitors.len() {
            let current = self.monitors[m_idx].current;
            for d_idx in 0..NUM_DESKTOPS {
                if d_idx == current {
                    continue;
                }
                let windows = self.monitors[m_idx].desktops[d_idx].clone();
                for &hwnd in &windows {
                    if is_valid_window(hwnd) {
                        set_window_visibility(hwnd, false, old_val);
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
                if lower_title.contains("task host window")
                    || lower_title.contains("windows push notifications")
                    || lower_title.contains("coremessaging")
                    || lower_title.contains("input experience")
                    || lower_title == "default ime"
                    || lower_title == "msctfime ui"
                    || lower_title == "popuphost"
                {
                    let mut one: i32 = 1;
                    DwmSetWindowAttribute(
                        hwnd,
                        DWMWA_CLOAK as _,
                        &mut one as *mut _ as _,
                        std::mem::size_of::<i32>() as u32,
                    );
                    ShowWindow(hwnd, SW_HIDE);
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
            log_info!(
                "  Space {} contains {} windows: {:?}",
                d_idx + 1,
                windows.len(),
                windows
            );
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
    }
}

pub fn set_window_visibility(hwnd: HWND, visible: bool, show_all_taskbar: bool) {
    unsafe {
        let mut state = get_window_state(hwnd);
        if state == 0 {
            return;
        }

        log_info!(
            "set_window_visibility: hwnd {:?} visible={} initial_state=0x{:X}",
            hwnd,
            visible,
            state
        );

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
