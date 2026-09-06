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
        if self.monitors[mon_idx].spaces.len() >= MAX_SPACES {
            return false;
        }
        self.monitors[mon_idx].spaces.push(Vec::new());
        self.monitors[mon_idx]
            .tiling
            .push(crate::tiling::TileSpace::new());
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
        mon.tiling.remove(space_idx);
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
        let ts = mon.tiling.remove(from_idx);
        mon.tiling.insert(to_idx, ts);

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
            self.monitors[mon_idx]
                .tiling
                .push(crate::tiling::TileSpace::new());
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
