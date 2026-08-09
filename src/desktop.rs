use crate::config::NUM_DESKTOPS;
use crate::log_info;
use std::ffi::c_void;
use std::ptr::{null, null_mut};
use windows_sys::Win32::Foundation::{BOOL, HWND, LPARAM, POINT, RECT};
use windows_sys::Win32::Graphics::Dwm::{DwmSetWindowAttribute, DWMWA_CLOAK};
use windows_sys::Win32::Graphics::Gdi::{
    EnumDisplayMonitors, MonitorFromPoint, MonitorFromWindow, HDC, HMONITOR,
    MONITOR_DEFAULTTONEAREST, MONITOR_DEFAULTTOPRIMARY,
};
use windows_sys::Win32::System::SystemInformation::GetTickCount;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::SetActiveWindow;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    BringWindowToTop, EnumWindows, GetCursorPos, GetForegroundWindow, GetPropA, GetTopWindow,
    GetWindow, GetWindowLongW, GetWindowThreadProcessId, IsIconic, IsWindow, IsWindowVisible,
    RemovePropA, SetForegroundWindow, SetPropA, SetWindowPos, ShowWindow, GWL_EXSTYLE, GWL_STYLE,
    GW_HWNDPREV, HWND_BOTTOM, HWND_TOPMOST, SWP_HIDEWINDOW, SWP_NOACTIVATE, SWP_NOMOVE,
    SWP_NOOWNERZORDER, SWP_NOSIZE, SWP_NOZORDER, SWP_SHOWWINDOW, SW_FORCEMINIMIZE, SW_HIDE,
    SW_SHOW, SW_SHOWMINNOACTIVE, SW_SHOWNOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_VISIBLE,
};

pub const MAX_MONITORS: usize = 8;
const WINSPACES_PROP_STATE: &[u8] = b"WinSpacesWindowState\0";

const WINSPACES_STATE_TRACKED: usize = 0x01;
const WINSPACES_STATE_WAS_ICONIC: usize = 0x02;
const WINSPACES_STATE_FORCED_MINIMIZED: usize = 0x04;
const WINSPACES_STATE_CLOAKED: usize = 0x08;

const SWP_BASE_FLAGS: u32 =
    SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_NOOWNERZORDER | SWP_NOZORDER;

#[derive(Debug, Clone)]
pub struct MonitorState {
    pub monitor: HMONITOR,
    pub current: usize,
    pub desktops: [Vec<HWND>; NUM_DESKTOPS],
    pub last_active: [Option<HWND>; NUM_DESKTOPS],
    pub z_order: [Vec<HWND>; NUM_DESKTOPS],
    pub suppress_foreground_until: u32,
}

impl MonitorState {
    pub fn new(monitor: HMONITOR) -> Self {
        Self {
            monitor,
            current: 0,
            desktops: Default::default(),
            last_active: Default::default(),
            z_order: Default::default(),
            suppress_foreground_until: 0,
        }
    }
}

pub struct DesktopManager {
    pub monitors: Vec<MonitorState>,
    pub show_all_taskbar: bool,
    pub handle_hotkeys: bool,
    pub suppress_foreground: bool,
}

impl DesktopManager {
    pub fn new() -> Self {
        let mut manager = Self {
            monitors: Vec::new(),
            show_all_taskbar: false,
            handle_hotkeys: true,
            suppress_foreground: false,
        };
        manager.refresh_monitors();
        manager
    }

    pub fn refresh_monitors(&mut self) {
        let mut monitors: Vec<HMONITOR> = Vec::new();
        unsafe {
            EnumDisplayMonitors(
                null_mut(),
                null(),
                Some(enum_monitors_callback),
                &mut monitors as *mut _ as LPARAM,
            );
        }

        if monitors.is_empty() {
            let primary =
                unsafe { MonitorFromPoint(POINT { x: 0, y: 0 }, MONITOR_DEFAULTTOPRIMARY) };
            monitors.push(primary);
        }

        self.monitors.retain(|m| monitors.contains(&m.monitor));
        for &mon in &monitors {
            if !self.monitors.iter().any(|m| m.monitor == mon) {
                self.monitors.push(MonitorState::new(mon));
            }
        }
        while self.monitors.len() > MAX_MONITORS {
            self.monitors.pop();
        }
    }

    pub fn find_monitor_index(&self, monitor: HMONITOR) -> usize {
        self.monitor_index_opt(monitor).unwrap_or(0)
    }

    fn monitor_index_opt(&self, monitor: HMONITOR) -> Option<usize> {
        self.monitors.iter().position(|m| m.monitor == monitor)
    }

    fn monitor_from_hwnd(&self, hwnd: HWND) -> usize {
        let monitor = unsafe { MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST) };
        self.find_monitor_index(monitor)
    }

    pub fn active_monitor(&self) -> usize {
        let mut pt = POINT { x: 0, y: 0 };
        if unsafe { GetCursorPos(&mut pt) } != 0 {
            let monitor = unsafe { MonitorFromPoint(pt, MONITOR_DEFAULTTONEAREST) };
            if let Some(idx) = self.monitor_index_opt(monitor) {
                return idx;
            }
        }
        let hwnd = unsafe { GetForegroundWindow() };
        if !hwnd.is_null() && is_valid_window(hwnd) {
            return self.monitor_from_hwnd(hwnd);
        }
        0
    }

    pub fn find_window(&self, hwnd: HWND) -> Option<(usize, usize)> {
        for (m, state) in self.monitors.iter().enumerate() {
            for (d, desk) in state.desktops.iter().enumerate() {
                if desk.contains(&hwnd) {
                    return Some((m, d));
                }
            }
        }
        None
    }

    pub fn contains_window(&self, hwnd: HWND) -> bool {
        self.find_window(hwnd).is_some()
    }

    pub fn update(&mut self) {
        self.refresh_monitors();

        let monitor_handles: Vec<HMONITOR> = self.monitors.iter().map(|m| m.monitor).collect();
        let monitor_count = self.monitors.len();
        let mut reassign: Vec<(HWND, usize)> = Vec::new();

        for m in 0..monitor_count {
            let current = self.monitors[m].current;
            for d in 0..NUM_DESKTOPS {
                let mut i = 0;
                while i < self.monitors[m].desktops[d].len() {
                    let hwnd = self.monitors[m].desktops[d][i];
                    if unsafe { GetWindowThreadProcessId(hwnd, null_mut()) } == 0 {
                        self.monitors[m].desktops[d].remove(i);
                        continue;
                    }
                    if d == current && unsafe { IsWindowVisible(hwnd) } == 0 {
                        self.monitors[m].desktops[d].remove(i);
                        continue;
                    }
                    let win_mon = unsafe { MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST) };
                    let new_idx = monitor_handles
                        .iter()
                        .position(|&h| h == win_mon)
                        .unwrap_or(0);
                    if new_idx != m {
                        self.monitors[m].desktops[d].remove(i);
                        reassign.push((hwnd, new_idx));
                        continue;
                    }
                    i += 1;
                }
            }
            for last in &mut self.monitors[m].last_active {
                if let Some(h) = *last {
                    if unsafe { IsWindow(h) } == 0 {
                        *last = None;
                    }
                }
            }
        }

        let show_all = self.show_all_taskbar;
        for (hwnd, new_idx) in reassign {
            if let Some(mon) = self.monitors.get_mut(new_idx) {
                let cur = mon.current;
                mon.desktops[cur].push(hwnd);
                set_window_visibility(hwnd, true, show_all);
            }
        }

        let mut found: Vec<HWND> = Vec::new();
        unsafe {
            EnumWindows(Some(enum_windows_callback), &mut found as *mut _ as LPARAM);
        }
        for &hwnd in &found {
            if self.contains_window(hwnd) {
                continue;
            }
            let mon_idx = self.monitor_from_hwnd(hwnd);
            if let Some(mon) = self.monitors.get_mut(mon_idx) {
                let cur = mon.current;
                mon.desktops[cur].push(hwnd);
                set_window_visibility(hwnd, true, show_all);
            }
        }
    }

    pub fn switch_desktop(&mut self, monitor_idx: usize, desk: usize, preferred: Option<HWND>) {
        if monitor_idx >= self.monitors.len() || desk >= NUM_DESKTOPS {
            return;
        }
        if self.monitors[monitor_idx].current == desk {
            return;
        }

        self.update();

        if monitor_idx >= self.monitors.len() {
            return;
        }
        if self.monitors[monitor_idx].current == desk {
            return;
        }

        let prev_suppress = self.suppress_foreground;
        self.suppress_foreground = true;
        let old = self.monitors[monitor_idx].current;

        if !prev_suppress {
            let fg = unsafe { GetForegroundWindow() };
            if !fg.is_null() && is_valid_window(fg) {
                if let Some((fg_mon, fg_desk)) = self.find_window(fg) {
                    if fg_mon == monitor_idx && fg_desk == old {
                        self.monitors[monitor_idx].last_active[old] = Some(fg);
                    }
                }
            }
        }

        let mut resolved = preferred;
        self.apply_desktop_visibility(monitor_idx, desk, true, resolved);
        self.monitors[monitor_idx].current = desk;
        self.apply_desktop_visibility(monitor_idx, old, false, None);

        self.suppress_foreground = prev_suppress;

        if !prev_suppress {
            self.monitors[monitor_idx].suppress_foreground_until = unsafe { GetTickCount() } + 250;
        }

        if resolved.is_none() {
            resolved = self.monitors[monitor_idx].last_active[desk];
        }
        if let Some(p) = resolved {
            if !p.is_null() && unsafe { IsIconic(p) } != 0 {
                resolved = None;
            }
        }
        if let Some(p) = resolved {
            match self.find_window(p) {
                Some((tm, td)) if tm == monitor_idx && td == desk => {}
                _ => resolved = None,
            }
        }
        if resolved.is_none() {
            let desk_wins = &self.monitors[monitor_idx].desktops[desk];
            for &candidate in desk_wins {
                if candidate.is_null() || unsafe { IsWindow(candidate) } == 0 {
                    continue;
                }
                if unsafe { IsIconic(candidate) } != 0 {
                    continue;
                }
                if unsafe { IsWindowVisible(candidate) } == 0 {
                    continue;
                }
                resolved = Some(candidate);
                break;
            }
        }
        if let Some(p) = resolved {
            activate_window(p);
            self.monitors[monitor_idx].last_active[desk] = Some(p);
        }
    }

    pub fn step_desktop(&mut self, delta: i32) {
        if self.monitors.is_empty() || delta == 0 {
            return;
        }
        let monitor_idx = self.active_monitor();
        if monitor_idx >= self.monitors.len() {
            return;
        }
        let current = self.monitors[monitor_idx].current;
        let step = ((delta.rem_euclid(NUM_DESKTOPS as i32)) as usize) % NUM_DESKTOPS;
        if step == 0 {
            return;
        }
        let target = (current + step) % NUM_DESKTOPS;
        self.switch_desktop(monitor_idx, target, None);
    }

    pub fn go_to_desk(&mut self, desk: usize) {
        if desk >= NUM_DESKTOPS {
            return;
        }
        self.switch_desktop(self.active_monitor(), desk, None);
    }

    pub fn move_to_desk(&mut self, desk: usize) {
        if desk >= NUM_DESKTOPS {
            return;
        }
        self.update();
        let hwnd = unsafe { GetForegroundWindow() };
        if hwnd.is_null() || !is_valid_window(hwnd) {
            return;
        }
        let monitor_idx = self.monitor_from_hwnd(hwnd);
        let current = self.monitors[monitor_idx].current;
        if current == desk {
            return;
        }
        self.monitors[monitor_idx].desktops[current].retain(|&h| h != hwnd);
        self.monitors[monitor_idx].desktops[desk].push(hwnd);
        let on_current = desk == self.monitors[monitor_idx].current;
        set_window_visibility(hwnd, on_current, self.show_all_taskbar);
    }

    pub fn move_and_switch_to_desk(&mut self, desk: usize) {
        if desk >= NUM_DESKTOPS {
            return;
        }
        let hwnd = unsafe { GetForegroundWindow() };
        if hwnd.is_null() || !is_valid_window(hwnd) {
            return;
        }
        let monitor_idx = self.monitor_from_hwnd(hwnd);
        let current = self.monitors[monitor_idx].current;
        if current == desk {
            return;
        }

        log_info!(
            "Moving window {:?} to Desktop {} and switching",
            hwnd,
            desk + 1
        );

        self.monitors[monitor_idx].desktops[current].retain(|&h| h != hwnd);
        self.monitors[monitor_idx].desktops[desk].push(hwnd);
        self.monitors[monitor_idx].current = desk;
        self.monitors[monitor_idx].last_active[desk] = Some(hwnd);
        self.apply_all_visibility(Some(hwnd));
        activate_window(hwnd);
    }

    pub fn step_move_window(&mut self, delta: i32) {
        if self.monitors.is_empty() || delta == 0 {
            return;
        }
        let hwnd = unsafe { GetForegroundWindow() };
        if hwnd.is_null() || !is_valid_window(hwnd) {
            return;
        }
        let monitor_idx = self.monitor_from_hwnd(hwnd);
        let current = self.monitors[monitor_idx].current;
        let target = if delta > 0 {
            (current + 1) % NUM_DESKTOPS
        } else {
            (current + NUM_DESKTOPS - 1) % NUM_DESKTOPS
        };
        self.move_and_switch_to_desk(target);
    }

    pub fn apply_all_visibility(&mut self, preferred_focus: Option<HWND>) {
        let monitor_count = self.monitors.len();
        for mon_idx in 0..monitor_count {
            let current_desk = self.monitors[mon_idx].current;
            for desk_idx in 0..NUM_DESKTOPS {
                let active = desk_idx == current_desk;
                let pref = if active {
                    preferred_focus.or(self.monitors[mon_idx].last_active[desk_idx])
                } else {
                    None
                };
                self.apply_desktop_visibility(mon_idx, desk_idx, active, pref);
            }
        }
    }

    fn apply_desktop_visibility(
        &mut self,
        mon_idx: usize,
        desk_idx: usize,
        active: bool,
        preferred: Option<HWND>,
    ) {
        if mon_idx >= self.monitors.len() || desk_idx >= NUM_DESKTOPS {
            return;
        }
        let show_all = self.show_all_taskbar;

        if active {
            // mem::take avoids cloning the saved z-order; the field ends up empty.
            let order = if !self.monitors[mon_idx].z_order[desk_idx].is_empty() {
                std::mem::take(&mut self.monitors[mon_idx].z_order[desk_idx])
            } else {
                self.collect_desk_windows(mon_idx, desk_idx)
            };
            let count = order.len();
            let desk_wins = &self.monitors[mon_idx].desktops[desk_idx];

            if count > 0 {
                for i in (0..count).rev() {
                    let hwnd = order[i];
                    if preferred == Some(hwnd) {
                        continue;
                    }
                    set_window_visibility(hwnd, true, show_all);
                }
            } else if !desk_wins.is_empty() {
                for i in (0..desk_wins.len()).rev() {
                    let hwnd = desk_wins[i];
                    if preferred == Some(hwnd) {
                        continue;
                    }
                    set_window_visibility(hwnd, true, show_all);
                }
            }
            if !desk_wins.is_empty() {
                for &hwnd in desk_wins {
                    if preferred == Some(hwnd) {
                        continue;
                    }
                    if count > 0 && order.contains(&hwnd) {
                        continue;
                    }
                    set_window_visibility(hwnd, true, show_all);
                }
            }
            if let Some(p) = preferred {
                set_window_visibility(p, true, show_all);
            }
            if count > 0 {
                restore_zorder(&order);
            }
            return;
        }

        self.monitors[mon_idx].z_order[desk_idx] = self.collect_desk_windows(mon_idx, desk_idx);
        let count = self.monitors[mon_idx].z_order[desk_idx].len();
        if count > 0 {
            for &hwnd in &self.monitors[mon_idx].z_order[desk_idx] {
                set_window_visibility(hwnd, false, show_all);
            }
        } else {
            for &hwnd in &self.monitors[mon_idx].desktops[desk_idx] {
                set_window_visibility(hwnd, false, show_all);
            }
        }
    }

    fn collect_desk_windows(&self, mon_idx: usize, desk_idx: usize) -> Vec<HWND> {
        let mut out: Vec<HWND> = Vec::new();
        let mut hwnd = unsafe { GetTopWindow(null_mut()) };
        while !hwnd.is_null() {
            if let Some((found_mon, found_desk)) = self.find_window(hwnd) {
                if found_mon == mon_idx && found_desk == desk_idx {
                    out.push(hwnd);
                }
            }
            hwnd = unsafe { GetWindow(hwnd, GW_HWNDPREV) };
        }
        out
    }

    pub fn set_show_all_taskbar(&mut self, enabled: bool) {
        if self.show_all_taskbar == enabled {
            return;
        }
        self.show_all_taskbar = enabled;
        self.apply_all_visibility(None);
    }

    pub fn windows_show_all(&mut self) {
        for m in 0..self.monitors.len() {
            for d in 0..NUM_DESKTOPS {
                for &hwnd in &self.monitors[m].desktops[d] {
                    set_window_cloak(hwnd, false);
                    set_window_state(hwnd, 0);
                    unsafe {
                        ShowWindow(hwnd, SW_SHOW);
                    }
                }
            }
        }
    }
}

extern "system" fn enum_monitors_callback(
    hmonitor: HMONITOR,
    _: HDC,
    _: *mut RECT,
    lparam: LPARAM,
) -> BOOL {
    unsafe {
        let list = &mut *(lparam as *mut Vec<HMONITOR>);
        list.push(hmonitor);
    }
    1
}

extern "system" fn enum_windows_callback(hwnd: HWND, lparam: LPARAM) -> BOOL {
    unsafe {
        if is_valid_window(hwnd) {
            let list = &mut *(lparam as *mut Vec<HWND>);
            list.push(hwnd);
        }
    }
    1
}

pub fn is_valid_window(hwnd: HWND) -> bool {
    if hwnd.is_null() || unsafe { IsWindow(hwnd) } == 0 {
        return false;
    }
    let ex_style = unsafe { GetWindowLongW(hwnd, GWL_EXSTYLE) } as u32;
    let style = unsafe { GetWindowLongW(hwnd, GWL_STYLE) } as u32;
    (style & WS_VISIBLE) != 0 && (ex_style & WS_EX_TOOLWINDOW) == 0
}

fn set_window_visibility(hwnd: HWND, active: bool, show_all_taskbar: bool) {
    if hwnd.is_null() || unsafe { IsWindow(hwnd) } == 0 {
        return;
    }
    let mut state = get_window_state(hwnd);
    let visible = unsafe { IsWindowVisible(hwnd) } != 0;
    let iconic = unsafe { IsIconic(hwnd) } != 0;

    if active {
        if (state & WINSPACES_STATE_CLOAKED) != 0 {
            set_window_cloak(hwnd, false);
            state &= !WINSPACES_STATE_CLOAKED;
        }
        if !visible {
            let failed = unsafe {
                SetWindowPos(
                    hwnd,
                    null_mut(),
                    0,
                    0,
                    0,
                    0,
                    SWP_BASE_FLAGS | SWP_SHOWWINDOW,
                )
            } == 0;
            if failed {
                let cmd = if (state & WINSPACES_STATE_WAS_ICONIC) != 0 {
                    SW_SHOWMINNOACTIVE
                } else {
                    SW_SHOWNOACTIVATE
                };
                unsafe {
                    ShowWindow(hwnd, cmd);
                }
            }
        }
        if (state & WINSPACES_STATE_FORCED_MINIMIZED) != 0 && unsafe { IsIconic(hwnd) } != 0 {
            let cmd = if (state & WINSPACES_STATE_WAS_ICONIC) != 0 {
                SW_SHOWMINNOACTIVE
            } else {
                SW_SHOWNOACTIVATE
            };
            unsafe {
                ShowWindow(hwnd, cmd);
            }
        }
        state &= !WINSPACES_STATE_FORCED_MINIMIZED;
        set_window_state(hwnd, state);
        return;
    }

    if iconic {
        state |= WINSPACES_STATE_WAS_ICONIC;
        state &= !WINSPACES_STATE_FORCED_MINIMIZED;
        if (state & WINSPACES_STATE_CLOAKED) != 0 {
            set_window_cloak(hwnd, false);
            state &= !WINSPACES_STATE_CLOAKED;
        }
        set_window_state(hwnd, state);
        return;
    }

    state &= !WINSPACES_STATE_WAS_ICONIC;
    state |= WINSPACES_STATE_TRACKED;
    if show_all_taskbar {
        if unsafe { IsIconic(hwnd) } == 0 {
            unsafe {
                ShowWindow(hwnd, SW_FORCEMINIMIZE);
            }
        }
        state |= WINSPACES_STATE_FORCED_MINIMIZED;
        if (state & WINSPACES_STATE_CLOAKED) != 0 {
            set_window_cloak(hwnd, false);
            state &= !WINSPACES_STATE_CLOAKED;
        }
    } else {
        if visible {
            let failed = unsafe {
                SetWindowPos(
                    hwnd,
                    null_mut(),
                    0,
                    0,
                    0,
                    0,
                    SWP_BASE_FLAGS | SWP_HIDEWINDOW,
                )
            } == 0;
            if failed {
                unsafe {
                    ShowWindow(hwnd, SW_HIDE);
                }
            }
        }
        if set_window_cloak(hwnd, true) {
            state |= WINSPACES_STATE_CLOAKED;
        } else {
            state &= !WINSPACES_STATE_CLOAKED;
        }
        state &= !WINSPACES_STATE_FORCED_MINIMIZED;
    }
    set_window_state(hwnd, state);
}

fn restore_zorder(windows: &[HWND]) {
    if windows.is_empty() {
        return;
    }
    let mut normal: Vec<HWND> = Vec::new();
    let mut topmost: Vec<HWND> = Vec::new();
    for i in (0..windows.len()).rev() {
        let hwnd = windows[i];
        if hwnd.is_null() || unsafe { IsWindow(hwnd) } == 0 {
            continue;
        }
        let ex = unsafe { GetWindowLongW(hwnd, GWL_EXSTYLE) } as u32;
        if (ex & WS_EX_TOPMOST) != 0 {
            topmost.push(hwnd);
        } else {
            normal.push(hwnd);
        }
    }
    if !normal.is_empty() {
        let mut insert_after = HWND_BOTTOM;
        for &hwnd in &normal {
            if unsafe { IsWindow(hwnd) } == 0 {
                continue;
            }
            unsafe {
                SetWindowPos(hwnd, insert_after, 0, 0, 0, 0, SWP_BASE_FLAGS);
            }
            insert_after = hwnd;
        }
    }
    if !topmost.is_empty() {
        let mut prev: HWND = null_mut();
        for i in (0..topmost.len()).rev() {
            let hwnd = topmost[i];
            if unsafe { IsWindow(hwnd) } == 0 {
                continue;
            }
            let target = if prev.is_null() { HWND_TOPMOST } else { prev };
            unsafe {
                SetWindowPos(hwnd, target, 0, 0, 0, 0, SWP_BASE_FLAGS);
            }
            prev = hwnd;
        }
    }
}

fn activate_window(hwnd: HWND) {
    if hwnd.is_null() || unsafe { IsWindow(hwnd) } == 0 {
        return;
    }
    if unsafe { IsIconic(hwnd) } != 0 {
        return;
    }
    unsafe {
        ShowWindow(hwnd, SW_SHOW);
        SetForegroundWindow(hwnd);
        BringWindowToTop(hwnd);
        SetActiveWindow(hwnd);
    }
}

fn set_window_cloak(hwnd: HWND, cloaked: bool) -> bool {
    let flag: u32 = if cloaked { 1 } else { 0 };
    let hr = unsafe {
        DwmSetWindowAttribute(
            hwnd,
            DWMWA_CLOAK as u32,
            &flag as *const _ as *const c_void,
            std::mem::size_of::<u32>() as u32,
        )
    };
    hr >= 0
}

fn get_window_state(hwnd: HWND) -> usize {
    (unsafe { GetPropA(hwnd, WINSPACES_PROP_STATE.as_ptr()) }) as usize
}

fn set_window_state(hwnd: HWND, state: usize) {
    if hwnd.is_null() || unsafe { IsWindow(hwnd) } == 0 {
        return;
    }
    if state != 0 {
        unsafe {
            SetPropA(hwnd, WINSPACES_PROP_STATE.as_ptr(), state as _);
        }
    } else {
        unsafe {
            RemovePropA(hwnd, WINSPACES_PROP_STATE.as_ptr());
        }
    }
}
