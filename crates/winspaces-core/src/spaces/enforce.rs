//! The settle window after a bulk placement and the post-restore
//! enforcement that pushes windows back where a topology restore put them
//! while the OS reconnect sweep is still fighting it. See
//! `docs/display-topology.md` for the timings.

use windows_sys::Win32::Foundation::HWND;
use windows_sys::Win32::System::SystemInformation::GetTickCount;
use winspaces_common::{log_info, WorkspaceRule};

use super::eligibility::is_live_window;
use super::index_math::tick_before;
use super::manager::SpaceManager;
use super::state::{get_window_state, WINSPACES_STATE_HIDDEN_MASK};

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

impl SpaceManager {
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
    pub(super) fn extend_restore_enforcement(&mut self) {
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
        if !self.is_enforcing_restore() || self.tiling_owns_window(hwnd) {
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
    pub(super) fn heal_restored_placement(&mut self, hwnd: HWND, mon_idx: usize, space_idx: usize) {
        if self.tiling_owns_window(hwnd) {
            return;
        }
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
            if !is_live_window(hwnd)
                || (get_window_state(hwnd) & WINSPACES_STATE_HIDDEN_MASK) != 0
                || self.tiling_owns_window(hwnd)
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
}
