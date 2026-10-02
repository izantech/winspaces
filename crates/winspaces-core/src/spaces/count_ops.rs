//! Per-monitor space-count operations: add, remove, reorder and bulk-set
//! spaces. The index arithmetic lives in `index_math`; this is the
//! bookkeeping over `monitors[].spaces` and the parallel `tiling` vector.

use winspaces_common::{log_info, MAX_SPACES};

use super::eligibility::is_valid_window;
use super::index_math::{
    remap_index_after_removal, remap_index_after_reorder, removal_migration_target,
};
use super::manager::SpaceManager;
use super::state::set_window_state;
use super::visibility::set_window_visibility;

impl SpaceManager {
    /// Append an empty space to a monitor. Does not switch to it (macOS
    /// doesn't either). Returns whether anything changed.
    pub fn add_space(&mut self, mon_idx: usize) -> bool {
        if mon_idx >= self.monitors.len() {
            return false;
        }
        if self.monitors[mon_idx].space_count() >= MAX_SPACES {
            return false;
        }
        self.insert_space_before_aux(mon_idx);
        log_info!(
            "add_space: Mon {} now has {} spaces",
            mon_idx + 1,
            self.monitors[mon_idx].space_count()
        );
        true
    }

    /// The aux shifts one right, and indices pointing at it follow.
    fn insert_space_before_aux(&mut self, mon_idx: usize) {
        let mon = &mut self.monitors[mon_idx];
        let at = mon.aux_idx();
        mon.spaces.insert(at, Vec::new());
        mon.tiling.insert(at, crate::tiling::TileSpace::new());
        if mon.current == at {
            mon.current += 1;
        }
        if mon.last_switched_space == at {
            mon.last_switched_space += 1;
        }
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
        let len = self.monitors[mon_idx].space_count();
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
        mon.tiling.remove(space_idx);
        mon.current = remap_index_after_removal(mon.current, space_idx);
        // last_switched_space feeds the taskbar-activation switchback; left
        // dangling it could target an out-of-range space.
        mon.last_switched_space = remap_index_after_removal(mon.last_switched_space, space_idx);
        mon.aux_return = remap_index_after_removal(mon.aux_return, space_idx);
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
        let len = self.monitors[mon_idx].space_count();
        if from_idx >= len || to_idx >= len || from_idx == to_idx {
            return false;
        }

        let mon = &mut self.monitors[mon_idx];
        let space = mon.spaces.remove(from_idx);
        mon.spaces.insert(to_idx, space);
        let ts = mon.tiling.remove(from_idx);
        mon.tiling.insert(to_idx, ts);

        mon.current = remap_index_after_reorder(mon.current, from_idx, to_idx);
        mon.last_switched_space =
            remap_index_after_reorder(mon.last_switched_space, from_idx, to_idx);
        mon.aux_return = remap_index_after_reorder(mon.aux_return, from_idx, to_idx);

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
        while self.monitors[mon_idx].space_count() < count {
            self.insert_space_before_aux(mon_idx);
        }
        while self.monitors[mon_idx].space_count() > count {
            let last = self.monitors[mon_idx].space_count() - 1;
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
            .map(|m| m.space_count())
            .max()
            .unwrap_or(1)
    }

    pub fn windows_show_all(&mut self) {
        log_info!("Restoring visibility for all managed windows");
        self.sticky_windows.clear();
        self.floating_windows.clear();
        self.auto_floated.clear();
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::spaces::MonitorState;
    use windows_sys::Win32::Foundation::HWND;

    const A: HWND = 100 as HWND;
    const B: HWND = 200 as HWND;
    const AUX_WIN: HWND = 300 as HWND;

    /// Spaces `[A] [B] []`, with `AUX_WIN` in the aux.
    fn manager() -> SpaceManager {
        let mut mgr = SpaceManager::for_test(vec![MonitorState::for_test(
            0,
            vec![vec![A], vec![B], vec![]],
        )]);
        let aux = mgr.monitors[0].aux_idx();
        mgr.monitors[0].spaces[aux].push(AUX_WIN);
        // Throttle the untracked-window scan: it prunes the fake handles.
        mgr.last_scan_tick =
            unsafe { windows_sys::Win32::System::SystemInformation::GetTickCount() };
        mgr
    }

    fn aux_windows(mgr: &SpaceManager) -> &[HWND] {
        let mon = &mgr.monitors[0];
        &mon.spaces[mon.aux_idx()]
    }

    #[test]
    fn the_aux_is_an_extra_slot_outside_the_count() {
        let mgr = manager();
        assert_eq!(mgr.monitors[0].space_count(), 3);
        assert_eq!(mgr.monitors[0].aux_idx(), 3);
        assert_eq!(mgr.max_space_count(), 3);
    }

    #[test]
    fn adding_a_space_keeps_the_aux_last_and_shown() {
        let mut mgr = manager();
        mgr.switch_space(0, 3, None);
        assert!(mgr.add_space(0));
        let mon = &mgr.monitors[0];
        assert_eq!(mon.space_count(), 4);
        assert_eq!(mon.spaces.len(), mon.tiling.len());
        assert!(mon.spaces[3].is_empty(), "the new space is empty");
        assert_eq!(aux_windows(&mgr), &[AUX_WIN]);
        assert!(mgr.monitors[0].in_aux(), "the aux stays on screen");
    }

    #[test]
    fn the_count_cap_ignores_the_aux() {
        let mut mgr = manager();
        mgr.set_space_count(0, MAX_SPACES);
        assert_eq!(mgr.monitors[0].space_count(), MAX_SPACES);
        assert!(!mgr.add_space(0));
        assert_eq!(aux_windows(&mgr), &[AUX_WIN]);
    }

    #[test]
    fn removing_spaces_never_touches_the_aux() {
        let mut mgr = manager();
        mgr.switch_space(0, 2, None);
        mgr.switch_space(0, 3, None);
        assert_eq!(mgr.monitors[0].aux_return, 2);

        assert!(mgr.remove_space(0, 0));
        let mon = &mgr.monitors[0];
        assert_eq!(mon.space_count(), 2);
        assert!(mon.in_aux());
        assert_eq!(mon.aux_return, 1, "follows the space it pointed at");
        assert_eq!(aux_windows(&mgr), &[AUX_WIN]);

        // The aux is not a space the count operations can remove or move.
        assert!(!mgr.remove_space(0, 2));
        assert!(!mgr.reorder_space(0, 2, 0));
        mgr.set_space_count(0, 1);
        assert_eq!(mgr.monitors[0].space_count(), 1);
        assert_eq!(aux_windows(&mgr), &[AUX_WIN]);
    }

    #[test]
    fn reordering_moves_the_return_space_with_its_windows() {
        let mut mgr = manager();
        mgr.switch_space(0, 3, None);
        assert_eq!(mgr.monitors[0].aux_return, 0);
        assert!(mgr.reorder_space(0, 0, 2));
        assert_eq!(mgr.monitors[0].aux_return, 2);
        assert_eq!(mgr.monitors[0].spaces[2], vec![A]);
    }

    #[test]
    fn toggling_the_aux_returns_to_the_space_it_covered() {
        let mut mgr = manager();
        mgr.switch_space(0, 1, None);
        mgr.toggle_aux_space();
        assert!(mgr.monitors[0].in_aux());
        assert_eq!(mgr.monitors[0].base_space(), 1);
        mgr.toggle_aux_space();
        assert_eq!(mgr.monitors[0].current, 1);
    }

    #[test]
    fn pinned_windows_show_on_the_aux_only_when_allowed() {
        let mut mgr = manager();
        mgr.sticky_windows.insert(A);
        mgr.switch_space(0, 3, None);
        assert!(mgr.should_be_visible(0, 0, A));
        assert!(mgr.windows_for_space(0, 3).contains(&A));

        mgr.pinned_in_aux = false;
        assert!(!mgr.should_be_visible(0, 0, A));
        assert_eq!(mgr.windows_for_space(0, 3), vec![AUX_WIN]);
        assert!(mgr.should_be_visible(0, 3, AUX_WIN));

        // Off the aux, the pin works as before.
        mgr.switch_space(0, 1, None);
        assert!(mgr.should_be_visible(0, 0, A));
    }

    #[test]
    fn persisted_counts_and_current_never_name_the_aux() {
        let mut mgr = manager();
        mgr.switch_space(0, 2, None);
        mgr.switch_space(0, 3, None);
        let live = crate::layout_store::live_monitors(&mgr);
        assert_eq!(live[0].space_count, 3);
        assert_eq!(live[0].current_space, 2);
        assert_eq!(mgr.monitors[0].clamp_space(7), 2);
    }
}
