//! Cross-monitor re-homing and adoption of untracked windows: the decisions
//! every window source (the scan, an activation, a drag) shares, so the
//! guards live in exactly one place.

use windows_sys::Win32::Foundation::HWND;
use winspaces_common::log_info;

use super::manager::SpaceManager;

/// Who noticed the window on another monitor. The scan and an activation
/// *infer* a move from geometry; a drop is the user's own gesture.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RehomeTrigger {
    Scan,
    Activation,
    MoveSize,
}

impl RehomeTrigger {
    /// Whether a settling bulk placement vetoes the move. The scan and an
    /// activation must not read not-yet-applied placement as a drag. A drop
    /// is different: every `flush_retile` arms the settle for 500 ms, and
    /// honouring it there swallowed most real drags and stranded the window
    /// on the target display while still tracked to the origin one. That is
    /// the "I can't move windows to my second monitor" symptom, and it
    /// compounds: the refusal snaps the window back, which dirties the space,
    /// which re-arms the settle for the retry.
    fn honours_settle(self) -> bool {
        !matches!(self, RehomeTrigger::MoveSize)
    }

    fn log_suffix(self) -> &'static str {
        match self {
            RehomeTrigger::Scan => "",
            RehomeTrigger::Activation => " on activation",
            RehomeTrigger::MoveSize => " via drag/movesize",
        }
    }
}

impl SpaceManager {
    /// `hwnd` is tracked on `from_mon` but sits on `to_mon`: move its
    /// tracking to `to_mon`'s current space unless the geometry is not the
    /// user's. Returns whether tracking changed.
    ///
    /// Re-homing lands the window on the target monitor's *current* space,
    /// which is right for a user drag but destroys space assignments
    /// wholesale when it fires during a topology change or right after a
    /// restore: there the window is somewhere transient, not somewhere the
    /// user put it. Hence the guards, in this order because the last one has
    /// side effects: a topology change in flight; a bulk placement still
    /// settling (see `RehomeTrigger::honours_settle`); and the restore
    /// enforcer, which pushes a just-restored window back instead of adopting
    /// the drift (adopting it would also poison the next shadow save).
    pub fn adopt_cross_monitor_move(
        &mut self,
        hwnd: HWND,
        from_mon: usize,
        to_mon: usize,
        trigger: RehomeTrigger,
    ) -> bool {
        if self.reconcile_pending {
            return false;
        }
        if trigger.honours_settle() && self.is_settling() {
            return false;
        }
        if self.try_enforce_restore(hwnd) {
            return false;
        }
        let space_idx = self.monitors[to_mon].current;
        log_info!(
            "Window {:?} moved across displays from Mon {} to Mon {} (Space {}){}",
            hwnd,
            from_mon + 1,
            to_mon + 1,
            space_idx + 1,
            trigger.log_suffix()
        );
        if trigger == RehomeTrigger::MoveSize {
            // The drag is over; the tiler must not classify it against the
            // origin monitor's layout.
            self.tiling_drag = None;
        }
        self.track_window(hwnd, to_mon, space_idx);
        true
    }

    /// Track an untracked window on the monitor it sits on, at that
    /// monitor's current space. `None` when the window resolves to no
    /// monitor (a stale HMONITOR mid-topology change): leaving it alone
    /// beats claiming it for the primary display.
    pub fn adopt_at_current(&mut self, hwnd: HWND) -> Option<(usize, usize)> {
        let mon_idx = self.monitor_index_for_hwnd(hwnd)?;
        let space_idx = self.monitors[mon_idx].current;
        self.track_window(hwnd, mon_idx, space_idx);
        Some((mon_idx, space_idx))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::spaces::MonitorState;

    const WINDOW: HWND = 100 as HWND;

    fn two_monitors() -> SpaceManager {
        SpaceManager::for_test(vec![
            MonitorState::for_test(0, vec![vec![WINDOW]]),
            MonitorState::for_test(1, vec![vec![]]),
        ])
    }

    #[test]
    fn a_topology_change_in_flight_blocks_every_trigger() {
        for trigger in [
            RehomeTrigger::Scan,
            RehomeTrigger::Activation,
            RehomeTrigger::MoveSize,
        ] {
            let mut mgr = two_monitors();
            mgr.reconcile_pending = true;
            assert!(!mgr.adopt_cross_monitor_move(WINDOW, 0, 1, trigger));
            assert_eq!(mgr.find_window(WINDOW), Some((0, 0)), "{trigger:?}");
        }
    }

    #[test]
    fn a_settling_placement_blocks_inferred_moves_only() {
        for trigger in [RehomeTrigger::Scan, RehomeTrigger::Activation] {
            let mut mgr = two_monitors();
            mgr.begin_settle(60_000);
            assert!(!mgr.adopt_cross_monitor_move(WINDOW, 0, 1, trigger));
            assert_eq!(mgr.find_window(WINDOW), Some((0, 0)), "{trigger:?}");
        }
    }

    #[test]
    fn a_drop_proceeds_while_settling() {
        // The user's own gesture wins over the settle window. With a
        // fabricated handle `track_window` finds no live window and untracks
        // it, so the observable outcome is "no longer tracked on monitor 1".
        let mut mgr = two_monitors();
        mgr.begin_settle(60_000);
        assert!(mgr.adopt_cross_monitor_move(WINDOW, 0, 1, RehomeTrigger::MoveSize));
        assert_eq!(mgr.find_window(WINDOW), None);
    }
}
