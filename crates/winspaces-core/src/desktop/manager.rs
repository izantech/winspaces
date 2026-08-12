//! `DesktopManager`: tracking, switching, and space-count operations.

use std::ptr::{null, null_mut};
use windows_sys::Win32::Foundation::{BOOL, HWND, LPARAM, POINT, RECT};
use windows_sys::Win32::Graphics::Dwm::{DwmSetWindowAttribute, DWMWA_CLOAK};
use windows_sys::Win32::Graphics::Gdi::{
    EnumDisplayMonitors, MonitorFromPoint, MonitorFromWindow, MONITOR_DEFAULTTONEAREST,
};
use windows_sys::Win32::System::SystemInformation::GetTickCount;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::SetActiveWindow;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GetCursorPos, GetForegroundWindow, GetWindowRect, GetWindowTextW, IsIconic,
    SetForegroundWindow, ShowWindow, SW_HIDE,
};
use winspaces_common::{log_info, log_warn, MAX_DESKTOPS};

use super::eligibility::{is_live_window, is_valid_window};
use super::index_math::{
    remap_index_after_removal, remap_index_after_reorder, removal_migration_target, tick_before,
};
use super::monitor::{enum_monitors_callback, EnumMonitorsContext, MonitorState};
use super::state::{
    get_window_state, set_window_state, WINSPACES_STATE_HIDDEN_MASK, WINSPACES_STATE_SYSTEM_HIDDEN,
    WINSPACES_STATE_TRACKED, WINSPACES_STATE_WAS_ICONIC,
};
use super::visibility::{set_window_visibility, AnimationGuard};

pub struct DesktopManager {
    pub monitors: Vec<MonitorState>,
    pub handle_hotkeys: bool,
    pub show_all_taskbar: bool,
    pub suppress_foreground: bool,
    /// Set between the first `WM_DISPLAYCHANGE` of a burst and the debounced
    /// reconcile that follows. While set, scans stop re-homing windows across
    /// monitors — the OS is mid-reflow and any conclusion drawn now is wrong.
    pub reconcile_pending: bool,
    /// Tick deadline after a bulk restore, during which cross-monitor re-homing
    /// is suppressed. `SetWindowPlacement`/`SetWindowPos` do not take effect
    /// synchronously — a scan moments later still sees the window at its old
    /// coordinates and "corrects" the tracking to match, dumping it on that
    /// monitor's current space. Observed 90 ms after a restore.
    pub suppress_rehome_until: u32,
    /// Tick of the last completed `scan_untracked_windows`, for the
    /// `SCAN_THROTTLE_MS` exemption in `switch_desktop`.
    last_scan_tick: u32,
}

impl Default for DesktopManager {
    fn default() -> Self {
        Self::new()
    }
}

impl DesktopManager {
    pub fn new() -> Self {
        // A previous instance may have died (crash, taskkill) leaving windows
        // cloaked/minimized with our state props still attached. Restore them
        // before scanning, otherwise they stay invisible forever.
        super::visibility::reclaim_orphaned_windows();
        let mut mgr = Self {
            monitors: Vec::new(),
            handle_hotkeys: true,
            show_all_taskbar: true,
            suppress_foreground: false,
            reconcile_pending: false,
            suppress_rehome_until: 0,
            last_scan_tick: 0,
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
            for d_idx in 0..self.monitors[m_idx].desktops.len() {
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
        // One display-config query for the whole enumeration: it walks every
        // active path, so doing it per monitor would be quadratic.
        let mut ctx = EnumMonitorsContext {
            monitors: Vec::new(),
            stable_ids: crate::topology::stable_monitor_ids(),
        };
        unsafe {
            EnumDisplayMonitors(
                null_mut(),
                null(),
                Some(enum_monitors_callback),
                &mut ctx as *mut _ as LPARAM,
            );
        }
        self.monitors = ctx.monitors;
    }

    /// Order-independent identity of the attached monitor set.
    pub fn topology_signature(&self) -> String {
        let ids: Vec<String> = self.monitors.iter().map(|m| m.stable_id.clone()).collect();
        crate::topology::signature_from_ids(&ids)
    }

    /// Resolve the monitor a window sits on. Prefers the `HMONITOR` identity,
    /// then falls back to locating the window's centre inside a monitor's
    /// bounds.
    ///
    /// The fallback matters: `MonitorFromWindow` can hand back a handle that is
    /// not in our table (reissued after a topology change, or a monitor beyond
    /// `MAX_MONITORS`). Callers previously treated that as "monitor 0", which
    /// silently dragged every window onto the primary display and onto that
    /// display's *current* space — destroying the user's space assignments with
    /// no display change involved. Returning `None` lets callers leave the
    /// window's tracking alone instead of corrupting it.
    ///
    /// `hwnd` is an opaque Win32 handle; `MonitorFromWindow`/`GetWindowRect`
    /// tolerate a stale or invalid one by failing gracefully, so this stays
    /// a safe fn despite carrying a raw-pointer-typed parameter.
    #[allow(clippy::not_unsafe_ptr_arg_deref)]
    pub fn monitor_index_for_hwnd(&self, hwnd: HWND) -> Option<usize> {
        unsafe {
            let hmon = MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST);
            if let Some(idx) = self.monitors.iter().position(|m| m.hmon == hmon) {
                return Some(idx);
            }

            let mut r: RECT = std::mem::zeroed();
            if GetWindowRect(hwnd, &mut r) == 0 {
                return None;
            }
            let centre = POINT {
                x: r.left + (r.right - r.left) / 2,
                y: r.top + (r.bottom - r.top) / 2,
            };
            self.monitors.iter().position(|m| m.contains(centre))
        }
    }

    /// Rebuild the monitor list after WM_DISPLAYCHANGE, carrying per-monitor
    /// space state over by stable device path (HMONITORs may be reissued, and
    /// `\\.\DISPLAYn` slot names get recycled onto entirely different physical
    /// monitors). Windows tracked on a vanished monitor are made visible and
    /// re-scanned onto whichever monitor the OS moved them to.
    pub fn handle_display_change(&mut self) {
        let old_monitors = std::mem::take(&mut self.monitors);
        self.update_monitors();
        log_info!(
            "Display change: {} -> {} monitors [{}]",
            old_monitors.len(),
            self.monitors.len(),
            self.monitors
                .iter()
                .map(|m| format!("{} {}", m.device, m.stable_id))
                .collect::<Vec<_>>()
                .join(", ")
        );

        let show_all = self.show_all_taskbar;
        for old in old_monitors {
            let new_idx = self
                .monitors
                .iter()
                .position(|m| m.stable_id == old.stable_id && !old.stable_id.is_empty());
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

    /// Hold off cross-monitor re-homing for `ms`. Call after any bulk
    /// placement: the scan must not treat not-yet-applied geometry as the user
    /// having dragged the window to another display.
    pub fn begin_settle(&mut self, ms: u32) {
        self.suppress_rehome_until = unsafe { GetTickCount() }.wrapping_add(ms);
    }

    pub fn is_settling(&self) -> bool {
        tick_before(unsafe { GetTickCount() }, self.suppress_rehome_until)
    }

    /// Show every window on each monitor's current space and hide the rest.
    /// Used after a bulk re-track (snapshot replay) where per-monitor `current`
    /// was set directly rather than by walking `switch_desktop`.
    pub fn reapply_visibility(&mut self) {
        let show_all = self.show_all_taskbar;
        let _no_anim = show_all.then(AnimationGuard::new);
        for mon in &self.monitors {
            for (d_idx, desk) in mon.desktops.iter().enumerate() {
                for &hwnd in desk {
                    if is_valid_window(hwnd) {
                        set_window_visibility(hwnd, d_idx == mon.current, show_all);
                    }
                }
            }
        }
    }

    /// Make every tracked window visible without disturbing space membership.
    /// The replay path needs this before moving windows: geometry applied to a
    /// cloaked or force-minimized window does not stick.
    pub fn show_all_tracked(&mut self) {
        let show_all = self.show_all_taskbar;
        for mon in &self.monitors {
            for desk in mon.desktops.iter() {
                for &hwnd in desk {
                    if is_valid_window(hwnd) {
                        set_window_visibility(hwnd, true, show_all);
                    }
                }
            }
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

    /// Drop `hwnd` from whichever space holds it, clearing its tracking state.
    ///
    /// Returns whether it was tracked at all. Called both when a window *moves*
    /// between spaces (via `track_window`) and when one is destroyed, which the
    /// daemon learns about from `HSHELL_WINDOWDESTROYED`.
    pub fn remove_window(&mut self, hwnd: HWND) -> bool {
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

    /// `hwnd` is an opaque Win32 handle; `IsIconic` tolerates a stale or
    /// invalid one by failing gracefully, so this stays a safe fn despite
    /// carrying a raw-pointer-typed parameter.
    #[allow(clippy::not_unsafe_ptr_arg_deref)]
    pub fn track_window(&mut self, hwnd: HWND, mon_idx: usize, desk_idx: usize) {
        self.remove_window(hwnd);

        if self.monitors.is_empty() || !is_valid_window(hwnd) {
            return;
        }

        // Workspace rules and IPC callers may reference a display that is not
        // currently attached (undocked laptop, powered-off screen). Fall back
        // to the window's actual monitor instead of indexing out of bounds —
        // this used to abort the daemon during startup rule restore. If the
        // window cannot be resolved either, leave it untracked rather than
        // dumping it on the primary display.
        let mon_idx = if mon_idx < self.monitors.len() {
            mon_idx
        } else {
            match self.monitor_index_for_hwnd(hwnd) {
                Some(fallback) => {
                    log_info!(
                        "track_window: Display {} not attached; falling back to Mon {}",
                        mon_idx + 1,
                        fallback + 1
                    );
                    fallback
                }
                None => {
                    log_warn!(
                        "track_window: Display {} not attached and hwnd {:?} resolves to no monitor; leaving untracked",
                        mon_idx + 1,
                        hwnd
                    );
                    return;
                }
            }
        };
        let desk_idx = desk_idx.min(self.monitors[mon_idx].desktops.len() - 1);

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

    /// Drop handles whose windows no longer exist.
    ///
    /// Nothing untracks a window that was simply closed: `remove_window` is
    /// only called when a window *moves* between spaces, and there is no
    /// destroy hook. `EnumWindows` cannot notice the gap either, since it only
    /// yields live windows. Dead handles therefore accumulate for the process
    /// lifetime and inflate per-space counts.
    ///
    /// Uses `is_live_window`, not `is_valid_window`: windows we hid on an
    /// inactive space are intentionally invisible and must survive the prune.
    ///
    /// Returns how many were dropped.
    pub fn prune_dead_windows(&mut self) -> usize {
        let mut dropped = 0;
        for mon in &mut self.monitors {
            for desk in &mut mon.desktops {
                let before = desk.len();
                desk.retain(|&h| is_live_window(h));
                dropped += before - desk.len();
            }
        }
        if dropped > 0 {
            log_info!("scan: pruned {} closed window(s) from tracking", dropped);
        }
        dropped
    }

    pub fn scan_untracked_windows(&mut self) {
        self.prune_dead_windows();

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
            if (state & WINSPACES_STATE_HIDDEN_MASK) != 0 {
                return 1;
            }

            if is_valid_window(hwnd) {
                // No resolvable monitor means a stale HMONITOR mid-topology
                // change. Leaving the window where it is beats claiming it for
                // the primary display.
                let Some(actual_mon_idx) = ctx.mgr.monitor_index_for_hwnd(hwnd) else {
                    log_warn!(
                        "scan: hwnd {:?} resolves to no monitor; leaving tracking unchanged",
                        hwnd
                    );
                    return 1;
                };

                match ctx.mgr.find_window(hwnd) {
                    None => {
                        let desk_idx = ctx.mgr.monitors[actual_mon_idx].current;
                        ctx.mgr.track_window(hwnd, actual_mon_idx, desk_idx);
                    }
                    Some((curr_mon, _curr_desk)) if curr_mon != actual_mon_idx => {
                        // Re-homing moves the window onto the target monitor's
                        // *current* space, which is correct for a user drag but
                        // destroys space assignments wholesale when it fires
                        // during a topology change or right after a restore —
                        // in both cases the window is somewhere transient, not
                        // somewhere the user put it.
                        if ctx.mgr.reconcile_pending || ctx.mgr.is_settling() {
                            return 1;
                        }
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
        self.last_scan_tick = unsafe { GetTickCount() };
    }

    pub fn go_to_desk(&mut self, target_desk: usize) {
        if self.monitors.is_empty() {
            return;
        }
        let mon_idx = self.get_active_monitor_index();
        // Bounds are per monitor: Alt+7 with the cursor on a 4-space monitor
        // is a deliberate no-op, not a clamp.
        if target_desk >= self.monitors[mon_idx].desktops.len() {
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
        let num = self.monitors[mon_idx].desktops.len() as i32;
        let next = (cur + delta).rem_euclid(num) as usize;
        self.go_to_desk(next);
    }

    pub fn move_to_desk(&mut self, target_desk: usize) {
        if self.monitors.is_empty() {
            return;
        }
        let fg = unsafe { GetForegroundWindow() };
        if fg.is_null() || !is_valid_window(fg) {
            return;
        }

        let mon_idx = self.get_active_monitor_index();
        if target_desk >= self.monitors[mon_idx].desktops.len() {
            return;
        }
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
        let num = self.monitors[mon_idx].desktops.len() as i32;
        let next = (cur + delta).rem_euclid(num) as usize;
        self.move_to_desk(next);
    }

    pub fn switch_desktop(
        &mut self,
        mon_idx: usize,
        target_desk: usize,
        activate_window: Option<HWND>,
    ) {
        if mon_idx >= self.monitors.len() || target_desk >= self.monitors[mon_idx].desktops.len() {
            return;
        }

        let now = unsafe { GetTickCount() };
        if !tick_before(
            now,
            self.last_scan_tick
                .wrapping_add(super::state::SCAN_THROTTLE_MS),
        ) {
            self.scan_untracked_windows();
        }

        let old_desk = self.monitors[mon_idx].current;

        log_info!(
            "switch_desktop: Mon {} from Space {} to Space {}",
            mon_idx + 1,
            old_desk + 1,
            target_desk + 1
        );

        self.monitors[mon_idx].last_switched_desk = old_desk;
        self.monitors[mon_idx].last_switch_time = now;
        self.monitors[mon_idx].current = target_desk;

        // Minimizing the old foreground makes the OS activate some other
        // window, and that event arrives asynchronously after this function
        // returns. Arm the guard on every monitor, not just the switching
        // one — the echo can land on a window tracked elsewhere and trigger
        // a phantom switch there.
        for mon in &mut self.monitors {
            mon.suppress_foreground_until = now.wrapping_add(500);
        }

        let show_all = self.show_all_taskbar;
        // The cloak path has no OS animation to suppress.
        let _no_anim = show_all.then(AnimationGuard::new);

        // Show the incoming space first, then drop the outgoing one: the
        // shows don't activate, so the new windows surface beneath the old
        // ones for a few frames instead of the desktop showing through.
        let target_windows = self.monitors[mon_idx].desktops[target_desk].clone();
        for &hwnd in &target_windows {
            if is_valid_window(hwnd) {
                set_window_visibility(hwnd, true, show_all);
            }
        }

        for d_idx in 0..self.monitors[mon_idx].desktops.len() {
            if d_idx == target_desk {
                continue;
            }
            let windows = self.monitors[mon_idx].desktops[d_idx].clone();
            for &hwnd in &windows {
                if is_valid_window(hwnd) {
                    set_window_visibility(hwnd, false, show_all);
                }
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

    /// Append an empty space to a monitor. Does not switch to it (macOS
    /// doesn't either). Returns whether anything changed.
    pub fn add_space(&mut self, mon_idx: usize) -> bool {
        if mon_idx >= self.monitors.len() {
            return false;
        }
        if self.monitors[mon_idx].desktops.len() >= MAX_DESKTOPS {
            return false;
        }
        self.monitors[mon_idx].desktops.push(Vec::new());
        log_info!(
            "add_space: Mon {} now has {} spaces",
            mon_idx + 1,
            self.monitors[mon_idx].desktops.len()
        );
        true
    }

    /// Remove space `desk_idx` from a monitor, migrating its windows to the
    /// space on the left (macOS semantics). Returns whether anything changed.
    ///
    /// Membership moves are pure Vec+prop operations via `track_window`; the
    /// single `switch_desktop` at the end is what resolves visibility — it
    /// re-shows the (possibly unchanged) current space and hides every other,
    /// which covers both "removed the current space" and "removed a background
    /// space whose windows migrated onto the current one".
    pub fn remove_space(&mut self, mon_idx: usize, desk_idx: usize) -> bool {
        if mon_idx >= self.monitors.len() {
            return false;
        }
        let len = self.monitors[mon_idx].desktops.len();
        if len <= 1 || desk_idx >= len {
            return false;
        }

        let occupants = self.monitors[mon_idx].desktops[desk_idx].clone();
        let target = removal_migration_target(desk_idx);
        for &hwnd in &occupants {
            self.track_window(hwnd, mon_idx, target);
        }

        let mon = &mut self.monitors[mon_idx];
        mon.desktops.remove(desk_idx);
        mon.current = remap_index_after_removal(mon.current, desk_idx);
        // last_switched_desk feeds the taskbar-activation switchback; left
        // dangling it could target an out-of-range space.
        mon.last_switched_desk = remap_index_after_removal(mon.last_switched_desk, desk_idx);
        let new_current = mon.current;
        log_info!(
            "remove_space: Mon {} removed Space {} ({} windows -> Space {}), {} spaces left",
            mon_idx + 1,
            desk_idx + 1,
            occupants.len(),
            target + 1,
            len - 1
        );

        self.switch_desktop(mon_idx, new_current, None);
        true
    }

    /// Move space `from_idx` to `to_idx` on monitor `mon_idx`.
    /// The windows on that space move with it, and active/last space indices
    /// are remapped. Returns whether anything changed.
    pub fn reorder_space(&mut self, mon_idx: usize, from_idx: usize, to_idx: usize) -> bool {
        if mon_idx >= self.monitors.len() {
            return false;
        }
        let len = self.monitors[mon_idx].desktops.len();
        if from_idx >= len || to_idx >= len || from_idx == to_idx {
            return false;
        }

        let mon = &mut self.monitors[mon_idx];
        let desk = mon.desktops.remove(from_idx);
        mon.desktops.insert(to_idx, desk);

        mon.current = remap_index_after_reorder(mon.current, from_idx, to_idx);
        mon.last_switched_desk =
            remap_index_after_reorder(mon.last_switched_desk, from_idx, to_idx);

        log_info!(
            "reorder_space: Mon {} moved Space {} to Space {} (active is now Space {})",
            mon_idx + 1,
            from_idx + 1,
            to_idx + 1,
            mon.current + 1,
        );
        true
    }

    /// Force a monitor to `count` spaces (clamped to `1..=MAX_DESKTOPS`).
    /// Used by snapshot restore; shrinking cascades windows down via
    /// `remove_space` so nothing is stranded on a deleted space.
    pub fn set_space_count(&mut self, mon_idx: usize, count: usize) {
        if mon_idx >= self.monitors.len() {
            return;
        }
        let count = count.clamp(1, MAX_DESKTOPS);
        while self.monitors[mon_idx].desktops.len() < count {
            self.monitors[mon_idx].desktops.push(Vec::new());
        }
        while self.monitors[mon_idx].desktops.len() > count {
            let last = self.monitors[mon_idx].desktops.len() - 1;
            if !self.remove_space(mon_idx, last) {
                break;
            }
        }
    }

    /// Highest space count across monitors — how many switch/move hotkeys
    /// need to be registered.
    pub fn max_space_count(&self) -> usize {
        self.monitors
            .iter()
            .map(|m| m.desktops.len())
            .max()
            .unwrap_or(1)
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
        super::visibility::reclaim_orphaned_windows();
    }
}
