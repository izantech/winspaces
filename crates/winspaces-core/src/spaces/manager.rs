//! `SpaceManager`: tracking, switching, and space-count operations.

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
use winspaces_common::{log_info, log_warn, WorkspaceRule, MAX_SPACES};

use super::eligibility::{is_live_window, is_valid_window};
use super::index_math::{
    remap_index_after_removal, remap_index_after_reorder, removal_migration_target, tick_before,
};
use super::monitor::{enum_monitors_callback, EnumMonitorsContext, MonitorState};
use super::notify::{notify_switch, SwitchNotice};
use super::state::{
    get_window_state, must_restore_before_untrack, set_window_state, WINSPACES_STATE_HIDDEN_MASK,
    WINSPACES_STATE_SYSTEM_HIDDEN, WINSPACES_STATE_TRACKED, WINSPACES_STATE_WAS_ICONIC,
};
use super::visibility::{set_window_visibility, AnimationGuard};

/// One placement applied by the last topology restore, kept so the scan and
/// the post-restore verify pass can push a window back when something moves it
/// right after the restore — Windows' "remember window locations" reconnect
/// sweep and some apps' own display-change handlers both reposition windows
/// seconds after a monitor returns, long after the placement settle expires.
pub struct RestoreTarget {
    pub hwnd: HWND,
    pub mon_idx: usize,
    pub space_idx: usize,
    pub rule: WorkspaceRule,
}

pub struct SpaceManager {
    pub monitors: Vec<MonitorState>,
    pub handle_hotkeys: bool,
    pub show_all_taskbar: bool,
    /// Whether a completed switch notifies the installed observer (the
    /// "Space N" indicator). Config-driven, like `show_all_taskbar`.
    pub space_indicator: bool,
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
    /// `SCAN_THROTTLE_MS` exemption in `switch_space`.
    last_scan_tick: u32,
    /// Placements from the last `restore_snapshot`, enforced (pushed back
    /// rather than re-homed) while `enforce_restore_until` is live.
    restore_targets: Vec<RestoreTarget>,
    enforce_restore_until: u32,
    /// The `ms` passed to `begin_restore_enforcement`, kept so each push-back
    /// can slide the deadline by the same amount.
    enforce_restore_ms: u32,
    /// Absolute ceiling for the sliding deadline. Without it, anything that
    /// keeps moving a window — including the user dragging it on purpose —
    /// would keep enforcement alive forever.
    enforce_restore_cap: u32,
}

impl Default for SpaceManager {
    fn default() -> Self {
        Self::new()
    }
}

impl SpaceManager {
    pub fn new() -> Self {
        // A previous instance may have died (crash, taskkill) leaving windows
        // cloaked/minimized with our state props still attached. Restore them
        // before scanning, otherwise they stay invisible forever.
        super::visibility::reclaim_orphaned_windows();
        let mut mgr = Self {
            monitors: Vec::new(),
            handle_hotkeys: true,
            show_all_taskbar: true,
            space_indicator: true,
            suppress_foreground: false,
            reconcile_pending: false,
            suppress_rehome_until: 0,
            last_scan_tick: 0,
            restore_targets: Vec::new(),
            enforce_restore_until: 0,
            enforce_restore_ms: 0,
            enforce_restore_cap: 0,
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
            for s_idx in 0..self.monitors[m_idx].spaces.len() {
                if s_idx == current {
                    continue;
                }
                let windows = self.monitors[m_idx].spaces[s_idx].clone();
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
                    self.monitors[idx].last_switched_space = old.last_switched_space;
                    self.monitors[idx].last_switch_time = old.last_switch_time;
                    self.monitors[idx].spaces = old.spaces;
                }
                None => {
                    for space in &old.spaces {
                        for &hwnd in space {
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

    /// Remember what the restore placed where, and enforce it for `ms`.
    ///
    /// The window is *sliding*: each successful push-back restarts it (the
    /// fight is evidently still on — the OS reconnect sweep has been observed
    /// re-moving a window ~450 ms after a fixed window expired), capped at
    /// four times `ms` so a user deliberately dragging a restored window
    /// cross-monitor loses at most that long, not forever.
    pub fn begin_restore_enforcement(&mut self, targets: Vec<RestoreTarget>, ms: u32) {
        self.restore_targets = targets;
        let now = unsafe { GetTickCount() };
        self.enforce_restore_ms = ms;
        self.enforce_restore_until = now.wrapping_add(ms);
        self.enforce_restore_cap = now.wrapping_add(ms.saturating_mul(4));
    }

    pub fn is_enforcing_restore(&self) -> bool {
        tick_before(unsafe { GetTickCount() }, self.enforce_restore_until)
    }

    /// Slide the enforcement deadline after a push-back, up to the cap.
    fn extend_restore_enforcement(&mut self) {
        let want = unsafe { GetTickCount() }.wrapping_add(self.enforce_restore_ms);
        self.enforce_restore_until = if tick_before(want, self.enforce_restore_cap) {
            want
        } else {
            self.enforce_restore_cap
        };
    }

    /// If enforcement is live and the last restore placed `hwnd`, re-apply that
    /// placement (and its tracking) instead of letting the caller adopt the
    /// window's drifted position. Returns whether a push-back happened.
    ///
    /// `hwnd` is an opaque Win32 handle, never dereferenced in Rust — it is
    /// only forwarded to `apply_rule_to_window`/`track_window`, which tolerate
    /// a stale one, so this stays a safe fn despite the raw-pointer parameter.
    #[allow(clippy::not_unsafe_ptr_arg_deref)]
    pub fn try_enforce_restore(&mut self, hwnd: HWND) -> bool {
        if !self.is_enforcing_restore() {
            return false;
        }
        let Some(target) = self.restore_targets.iter().find(|t| t.hwnd == hwnd) else {
            return false;
        };
        let (mon_idx, space_idx) = (target.mon_idx, target.space_idx);
        if mon_idx >= self.monitors.len() {
            return false;
        }
        let rule = target.rule.clone();
        let hmon = self.monitors[mon_idx].hmon;
        unsafe {
            crate::workspaces::apply_rule_to_window(hwnd, &rule, Some(hmon));
        }
        self.track_window(hwnd, mon_idx, space_idx);
        self.extend_restore_enforcement();
        log_info!(
            "restore-enforce: pushed hwnd {:?} back to Mon {}, Space {}",
            hwnd,
            mon_idx + 1,
            space_idx + 1
        );
        true
    }

    /// A restored window shown by a space switch may have been moved by the OS
    /// reconnect sweep *while cloaked* — invisible to the scan (hidden windows
    /// return early) and to the verify sweep (which skips hidden windows on
    /// purpose). The uncloak does not re-assert geometry, so the window would
    /// surface wherever its rect drifted to. Heal it here instead, with no
    /// time window: a cloaked window cannot have been user-dragged, and the
    /// scan at the top of `switch_space` re-homes genuine drags before this
    /// runs, which makes the tracking-must-match-the-target condition below a
    /// reliable staleness guard.
    fn heal_restored_placement(&mut self, hwnd: HWND, mon_idx: usize, space_idx: usize) {
        let Some(target) = self
            .restore_targets
            .iter()
            .find(|t| t.hwnd == hwnd && t.mon_idx == mon_idx && t.space_idx == space_idx)
        else {
            return;
        };
        if self.monitor_index_for_hwnd(hwnd) == Some(mon_idx) {
            return;
        }
        let rule = target.rule.clone();
        let hmon = self.monitors[mon_idx].hmon;
        unsafe {
            crate::workspaces::apply_rule_to_window(hwnd, &rule, Some(hmon));
        }
        // A heal means something moved this window while it was cloaked —
        // the same fight the enforcement window exists for. Keep it open.
        self.extend_restore_enforcement();
        log_info!(
            "restore-heal: hwnd {:?} surfaced off its restored monitor; moved back to Mon {}",
            hwnd,
            mon_idx + 1
        );
    }

    /// Sweep every restore target and push back any visible window that has
    /// drifted off its restored monitor. Hidden windows are skipped — geometry
    /// does not reliably stick to a cloaked window; they are healed at show
    /// time by `heal_restored_placement` instead.
    pub fn enforce_restore_pass(&mut self) -> usize {
        if !self.is_enforcing_restore() {
            return 0;
        }
        let candidates: Vec<(HWND, usize)> = self
            .restore_targets
            .iter()
            .map(|t| (t.hwnd, t.mon_idx))
            .collect();
        let mut pushed = 0usize;
        for (hwnd, target_mon) in candidates {
            if !is_live_window(hwnd) || (get_window_state(hwnd) & WINSPACES_STATE_HIDDEN_MASK) != 0
            {
                continue;
            }
            if let Some(actual) = self.monitor_index_for_hwnd(hwnd) {
                if actual != target_mon && self.try_enforce_restore(hwnd) {
                    pushed += 1;
                }
            }
        }
        pushed
    }

    /// Show every window on each monitor's current space and hide the rest.
    /// Used after a bulk re-track (snapshot replay) where per-monitor `current`
    /// was set directly rather than by walking `switch_space`.
    pub fn reapply_visibility(&mut self) {
        let show_all = self.show_all_taskbar;
        let _no_anim = show_all.then(AnimationGuard::new);
        for mon in &self.monitors {
            for (s_idx, space) in mon.spaces.iter().enumerate() {
                for &hwnd in space {
                    if is_valid_window(hwnd) {
                        set_window_visibility(hwnd, s_idx == mon.current, show_all);
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
            for space in mon.spaces.iter() {
                for &hwnd in space {
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
            for (s_idx, space) in mon.spaces.iter().enumerate() {
                if space.contains(&hwnd) {
                    return Some((m_idx, s_idx));
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
            for space in &mut mon.spaces {
                if let Some(pos) = space.iter().position(|&h| h == hwnd) {
                    space.remove(pos);
                    removed = true;
                }
            }
        }
        if removed {
            set_window_state(hwnd, 0);
            crate::workspaces::identity::invalidate_identity(hwnd);
        }
        removed
    }

    /// `hwnd` is an opaque Win32 handle; `IsIconic` tolerates a stale or
    /// invalid one by failing gracefully, so this stays a safe fn despite
    /// carrying a raw-pointer-typed parameter.
    #[allow(clippy::not_unsafe_ptr_arg_deref)]
    pub fn track_window(&mut self, hwnd: HWND, mon_idx: usize, space_idx: usize) {
        // Read the state and judge eligibility BEFORE `remove_window` clears
        // the prop: with the hidden bits gone, a window we cloaked reads as
        // externally cloaked and fails `is_valid_window`, so re-tracking a
        // hidden window (workspace restore, MC drag between background
        // spaces) would leave it cloaked, untracked, and prop-less — stuck
        // invisible with nothing left that knows how to bring it back.
        let prev_state = get_window_state(hwnd);
        if self.monitors.is_empty() || !is_valid_window(hwnd) {
            // Untracking is not enough: `remove_window` clears the prop, and
            // that prop is the *only* record that we applied the cloak. Drop
            // it while the cloak is physically on and the window is stranded
            // for good — the show path early-returns on `state == 0`, the
            // eligibility probe then reads it as *externally* cloaked (so it
            // can never become valid again), and `reclaim_orphaned_windows`
            // skips it at exit and startup because it has no prop left to
            // find. Restore it here, while the bits still say how.
            if must_restore_before_untrack(prev_state, is_live_window(hwnd)) {
                set_window_visibility(hwnd, true, self.show_all_taskbar);
            }
            self.remove_window(hwnd);
            return;
        }
        self.remove_window(hwnd);

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
        let space_idx = space_idx.min(self.monitors[mon_idx].spaces.len() - 1);

        // A hidden window stays hidden across a re-track: the cloak is still
        // physically applied, so the bits that say "we did this" must survive,
        // along with the pre-hide iconic flag the eventual show will replay.
        let mut state = WINSPACES_STATE_TRACKED | (prev_state & WINSPACES_STATE_HIDDEN_MASK);
        if (prev_state & WINSPACES_STATE_HIDDEN_MASK) != 0 {
            state |= prev_state & WINSPACES_STATE_WAS_ICONIC;
        } else {
            unsafe {
                if IsIconic(hwnd) != 0 {
                    state |= WINSPACES_STATE_WAS_ICONIC;
                }
            }
        }
        set_window_state(hwnd, state);
        self.monitors[mon_idx].spaces[space_idx].push(hwnd);
        log_info!(
            "track_window: hwnd {:?} -> Mon {}, Space {}",
            hwnd,
            mon_idx + 1,
            space_idx + 1
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
            for space in &mut mon.spaces {
                let before = space.len();
                space.retain(|&h| {
                    if is_live_window(h) {
                        true
                    } else {
                        crate::workspaces::identity::invalidate_identity(h);
                        false
                    }
                });
                dropped += before - space.len();
            }
        }
        if dropped > 0 {
            log_info!("scan: pruned {} closed window(s) from tracking", dropped);
        }
        // Restore targets for dead handles must go too: Win32 recycles handle
        // values, and a recycled handle matching a stale target could get
        // yanked to the old monitor by the show-time heal.
        self.restore_targets.retain(|t| is_live_window(t.hwnd));
        dropped
    }

    pub fn scan_untracked_windows(&mut self) {
        self.prune_dead_windows();

        struct ScanContext<'a> {
            mgr: &'a mut SpaceManager,
        }

        /// Exact titles only: a substring match here would cloak legitimate
        /// user windows (e.g. a browser tab mentioning "input experience").
        /// Compared case-insensitively against the raw UTF-16 buffer — the
        /// list is pure ASCII, and this callback runs for every top-level
        /// window on every scan, so it must not allocate.
        fn is_system_shell_title(units: &[u16]) -> bool {
            const SYSTEM_TITLES: &[&str] = &[
                "windows input experience",
                "experiencia de entrada de windows",
                "task host window",
                "windows push notifications platform",
                "coremessaging",
                "default ime",
                "msctfime ui",
                "popuphost",
            ];
            SYSTEM_TITLES.iter().any(|s| {
                units.len() == s.len()
                    && units
                        .iter()
                        .zip(s.bytes())
                        .all(|(&u, b)| u < 0x80 && (u as u8).eq_ignore_ascii_case(&b))
            })
        }

        unsafe extern "system" fn enum_windows_proc(hwnd: HWND, lparam: LPARAM) -> BOOL {
            let ctx = &mut *(lparam as *mut ScanContext);

            // One `GetPropA` up front: windows we hid need no further work,
            // and the answer gates everything below.
            let state = get_window_state(hwnd);
            if (state & WINSPACES_STATE_HIDDEN_MASK) != 0 {
                return 1;
            }

            // Tracked windows passed structural eligibility, so they can never
            // be one of the exact-title system windows; the title fetch is
            // only needed for unknown windows.
            if (state & WINSPACES_STATE_TRACKED) == 0 {
                let mut title = [0u16; 256];
                let len = GetWindowTextW(hwnd, title.as_mut_ptr(), title.len() as i32);
                if len > 0 && is_system_shell_title(&title[..len as usize]) {
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
                        let space_idx = ctx.mgr.monitors[actual_mon_idx].current;
                        ctx.mgr.track_window(hwnd, actual_mon_idx, space_idx);
                    }
                    Some((curr_mon, _curr_space)) if curr_mon != actual_mon_idx => {
                        // Re-homing moves the window onto the target monitor's
                        // *current* space, which is correct for a user drag but
                        // destroys space assignments wholesale when it fires
                        // during a topology change or right after a restore —
                        // in both cases the window is somewhere transient, not
                        // somewhere the user put it.
                        if ctx.mgr.reconcile_pending || ctx.mgr.is_settling() {
                            return 1;
                        }
                        // Right after a topology restore, a cross-monitor move
                        // is the OS reconnect sweep (or the app itself) fighting
                        // the restore, not a user drag: push the window back to
                        // where the restore put it instead of adopting the
                        // drift — adopting it also poisons the next shadow save.
                        if ctx.mgr.try_enforce_restore(hwnd) {
                            return 1;
                        }
                        let space_idx = ctx.mgr.monitors[actual_mon_idx].current;
                        log_info!(
                            "Window {:?} moved across displays from Mon {} to Mon {} (Space {})",
                            hwnd,
                            curr_mon + 1,
                            actual_mon_idx + 1,
                            space_idx + 1
                        );
                        ctx.mgr.track_window(hwnd, actual_mon_idx, space_idx);
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

    pub fn go_to_space(&mut self, target_space: usize) {
        if self.monitors.is_empty() {
            return;
        }
        let mon_idx = self.get_active_monitor_index();
        // Bounds are per monitor: Alt+7 with the cursor on a 4-space monitor
        // is a deliberate no-op, not a clamp.
        if target_space >= self.monitors[mon_idx].spaces.len() {
            return;
        }

        if self.monitors[mon_idx].current == target_space {
            return;
        }

        self.switch_space(mon_idx, target_space, None);
    }

    pub fn step_space(&mut self, delta: i32) {
        if self.monitors.is_empty() {
            return;
        }
        let mon_idx = self.get_active_monitor_index();
        let cur = self.monitors[mon_idx].current as i32;
        let num = self.monitors[mon_idx].spaces.len() as i32;
        let next = (cur + delta).rem_euclid(num) as usize;
        self.go_to_space(next);
    }

    pub fn move_to_space(&mut self, target_space: usize) {
        if self.monitors.is_empty() {
            return;
        }
        let fg = unsafe { GetForegroundWindow() };
        if fg.is_null() || !is_valid_window(fg) {
            return;
        }

        let mon_idx = self.get_active_monitor_index();
        if target_space >= self.monitors[mon_idx].spaces.len() {
            return;
        }
        let cur = self.monitors[mon_idx].current;
        if cur == target_space {
            return;
        }

        self.track_window(fg, mon_idx, target_space);
        self.switch_space(mon_idx, target_space, Some(fg));
    }

    pub fn step_move_window(&mut self, delta: i32) {
        if self.monitors.is_empty() {
            return;
        }
        let mon_idx = self.get_active_monitor_index();
        let cur = self.monitors[mon_idx].current as i32;
        let num = self.monitors[mon_idx].spaces.len() as i32;
        let next = (cur + delta).rem_euclid(num) as usize;
        self.move_to_space(next);
    }

    pub fn switch_space(
        &mut self,
        mon_idx: usize,
        target_space: usize,
        activate_window: Option<HWND>,
    ) {
        if mon_idx >= self.monitors.len() || target_space >= self.monitors[mon_idx].spaces.len() {
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

        let old_space = self.monitors[mon_idx].current;

        log_info!(
            "switch_space: Mon {} from Space {} to Space {}",
            mon_idx + 1,
            old_space + 1,
            target_space + 1
        );

        self.monitors[mon_idx].last_switched_space = old_space;
        self.monitors[mon_idx].last_switch_time = now;
        self.monitors[mon_idx].current = target_space;

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
        let target_windows = self.monitors[mon_idx].spaces[target_space].clone();
        for &hwnd in &target_windows {
            if is_valid_window(hwnd) {
                set_window_visibility(hwnd, true, show_all);
                self.heal_restored_placement(hwnd, mon_idx, target_space);
            }
        }

        for s_idx in 0..self.monitors[mon_idx].spaces.len() {
            if s_idx == target_space {
                continue;
            }
            for &hwnd in &self.monitors[mon_idx].spaces[s_idx] {
                // Everything not on the outgoing space is already hidden;
                // one `GetProp` answers that without the eligibility probe's
                // cross-process DWM query. The sweep must still cover every
                // non-target space — Mission Control drag-drop parks visible
                // windows on background spaces, and `remove_space` relies on
                // the full sweep with `old_space == target_space`.
                if (get_window_state(hwnd) & WINSPACES_STATE_HIDDEN_MASK) != 0 {
                    continue;
                }
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

        // Notify last, so an observer that paints sees the switch already
        // settled. The `old_space != target_space` guard is what keeps the
        // indicator off the two callers that switch to the space already
        // current: the workspace restore pass (which re-issues a switch purely
        // to re-apply visibility) and `remove_space` (whose `current` is
        // already remapped by the time it gets here).
        if self.space_indicator && old_space != target_space {
            let mon = &self.monitors[mon_idx];
            notify_switch(&SwitchNotice {
                mon_idx,
                space_idx: target_space,
                space_count: mon.spaces.len(),
                work: mon.work,
            });
        }
    }

    /// Append an empty space to a monitor. Does not switch to it (macOS
    /// doesn't either). Returns whether anything changed.
    pub fn add_space(&mut self, mon_idx: usize) -> bool {
        if mon_idx >= self.monitors.len() {
            return false;
        }
        if self.monitors[mon_idx].spaces.len() >= MAX_SPACES {
            return false;
        }
        self.monitors[mon_idx].spaces.push(Vec::new());
        log_info!(
            "add_space: Mon {} now has {} spaces",
            mon_idx + 1,
            self.monitors[mon_idx].spaces.len()
        );
        true
    }

    /// Remove space `space_idx` from a monitor, migrating its windows to the
    /// space on the left (macOS semantics). Returns whether anything changed.
    ///
    /// Membership moves are pure Vec+prop operations via `track_window`; the
    /// single `switch_space` at the end is what resolves visibility — it
    /// re-shows the (possibly unchanged) current space and hides every other,
    /// which covers both "removed the current space" and "removed a background
    /// space whose windows migrated onto the current one".
    pub fn remove_space(&mut self, mon_idx: usize, space_idx: usize) -> bool {
        if mon_idx >= self.monitors.len() {
            return false;
        }
        let len = self.monitors[mon_idx].spaces.len();
        if len <= 1 || space_idx >= len {
            return false;
        }

        let occupants = self.monitors[mon_idx].spaces[space_idx].clone();
        let target = removal_migration_target(space_idx);
        for &hwnd in &occupants {
            self.track_window(hwnd, mon_idx, target);
        }

        let mon = &mut self.monitors[mon_idx];
        mon.spaces.remove(space_idx);
        mon.current = remap_index_after_removal(mon.current, space_idx);
        // last_switched_space feeds the taskbar-activation switchback; left
        // dangling it could target an out-of-range space.
        mon.last_switched_space = remap_index_after_removal(mon.last_switched_space, space_idx);
        let new_current = mon.current;
        log_info!(
            "remove_space: Mon {} removed Space {} ({} windows -> Space {}), {} spaces left",
            mon_idx + 1,
            space_idx + 1,
            occupants.len(),
            target + 1,
            len - 1
        );

        self.switch_space(mon_idx, new_current, None);
        true
    }

    /// Move space `from_idx` to `to_idx` on monitor `mon_idx`.
    /// The windows on that space move with it, and active/last space indices
    /// are remapped. Returns whether anything changed.
    pub fn reorder_space(&mut self, mon_idx: usize, from_idx: usize, to_idx: usize) -> bool {
        if mon_idx >= self.monitors.len() {
            return false;
        }
        let len = self.monitors[mon_idx].spaces.len();
        if from_idx >= len || to_idx >= len || from_idx == to_idx {
            return false;
        }

        let mon = &mut self.monitors[mon_idx];
        let space = mon.spaces.remove(from_idx);
        mon.spaces.insert(to_idx, space);

        mon.current = remap_index_after_reorder(mon.current, from_idx, to_idx);
        mon.last_switched_space =
            remap_index_after_reorder(mon.last_switched_space, from_idx, to_idx);

        log_info!(
            "reorder_space: Mon {} moved Space {} to Space {} (active is now Space {})",
            mon_idx + 1,
            from_idx + 1,
            to_idx + 1,
            mon.current + 1,
        );
        true
    }

    /// Force a monitor to `count` spaces (clamped to `1..=MAX_SPACES`).
    /// Used by snapshot restore; shrinking cascades windows down via
    /// `remove_space` so nothing is stranded on a deleted space.
    pub fn set_space_count(&mut self, mon_idx: usize, count: usize) {
        if mon_idx >= self.monitors.len() {
            return;
        }
        let count = count.clamp(1, MAX_SPACES);
        while self.monitors[mon_idx].spaces.len() < count {
            self.monitors[mon_idx].spaces.push(Vec::new());
        }
        while self.monitors[mon_idx].spaces.len() > count {
            let last = self.monitors[mon_idx].spaces.len() - 1;
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
            .map(|m| m.spaces.len())
            .max()
            .unwrap_or(1)
    }

    pub fn windows_show_all(&mut self) {
        log_info!("Restoring visibility for all managed windows");
        let show_all = self.show_all_taskbar;
        for mon in &mut self.monitors {
            for space in &mut mon.spaces {
                for &hwnd in space.iter() {
                    if is_valid_window(hwnd) {
                        set_window_visibility(hwnd, true, show_all);
                        set_window_state(hwnd, 0);
                    }
                }
                space.clear();
            }
        }
        // Catch anything the tracked lists missed: system windows we cloaked
        // and windows whose validity changed since tracking.
        super::visibility::reclaim_orphaned_windows();
    }
}
