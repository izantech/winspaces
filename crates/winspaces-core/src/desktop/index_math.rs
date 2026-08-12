//! Pure index arithmetic for space add/remove/reorder. No Win32 — these were
//! interleaved between `MonitorState`'s struct and its impl in the
//! pre-split desktop.rs; that co-location was incidental, not structural.

/// Wrapping-safe "`now` has not yet reached `deadline`". `GetTickCount` rolls
/// over every ~49 days, so a plain `<` breaks once per rollover. A zero
/// deadline means "no deadline set" — without that guard, any machine up for
/// more than 24.8 days would read as permanently settling.
pub(crate) fn tick_before(now: u32, deadline: u32) -> bool {
    deadline != 0 && now.wrapping_sub(deadline) > u32::MAX / 2
}

/// Space that inherits the windows of a removed space, in the indexing that
/// is live *while the removed space still exists* (`track_window` runs before
/// the `Vec::remove`). macOS semantics: occupants go to the space on the
/// left; the first space has no left neighbour, so its windows fall right
/// onto old space 1 — which becomes space 0 once the removal shifts.
pub(crate) fn removal_migration_target(removed: usize) -> usize {
    if removed == 0 {
        1
    } else {
        removed - 1
    }
}

/// Where a stored space index points after space `removed` has been deleted.
/// An index *on* the removed space follows its migrated windows left.
pub(crate) fn remap_index_after_removal(idx: usize, removed: usize) -> usize {
    if idx > removed {
        idx - 1
    } else if idx == removed {
        idx.saturating_sub(1)
    } else {
        idx
    }
}

/// Where a stored space index points after space `from` has been moved to `to`.
pub fn remap_index_after_reorder(idx: usize, from: usize, to: usize) -> usize {
    if idx == from {
        to
    } else if from < to {
        if idx > from && idx <= to {
            idx - 1
        } else {
            idx
        }
    } else if from > to {
        if idx >= to && idx < from {
            idx + 1
        } else {
            idx
        }
    } else {
        idx
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settle_deadline_is_live_until_it_passes() {
        assert!(tick_before(1_000, 5_000));
        assert!(!tick_before(5_000, 5_000));
        assert!(!tick_before(9_000, 5_000));
    }

    #[test]
    fn settle_deadline_survives_tick_rollover() {
        // begin_settle near the 49-day rollover wraps the deadline past zero;
        // a plain `now < deadline` would report "already expired" and let the
        // scan re-home windows mid-restore.
        let now = u32::MAX - 1_000;
        let deadline = now.wrapping_add(4_000); // wraps to ~2999
        assert!(tick_before(now, deadline));
        assert!(tick_before(u32::MAX, deadline));
        assert!(!tick_before(3_000, deadline));
    }

    #[test]
    fn zero_deadline_never_settles_however_long_the_uptime() {
        // 30 days of uptime: `now` alone exceeds u32::MAX/2.
        assert!(!tick_before(30 * 24 * 60 * 60 * 1000, 0));
        assert!(!tick_before(0, 0));
    }

    #[test]
    fn removed_space_windows_go_left_and_first_space_falls_right() {
        assert_eq!(removal_migration_target(3), 2);
        assert_eq!(removal_migration_target(1), 0);
        // No left neighbour: old space 1 inherits, which becomes space 0
        // after the shift.
        assert_eq!(removal_migration_target(0), 1);
    }

    #[test]
    fn indices_remap_around_a_removed_space() {
        // Before the removed space: untouched.
        assert_eq!(remap_index_after_removal(1, 3), 1);
        // On the removed space: follows the migrated windows left.
        assert_eq!(remap_index_after_removal(3, 3), 2);
        assert_eq!(remap_index_after_removal(0, 0), 0);
        // Past the removed space: shifts down by one.
        assert_eq!(remap_index_after_removal(5, 3), 4);
        assert_eq!(remap_index_after_removal(1, 0), 0);
    }

    #[test]
    fn indices_remap_around_a_reordered_space() {
        // Moving right: from 1 to 3
        // Target:
        assert_eq!(remap_index_after_reorder(1, 1, 3), 3);
        // Untouched left of from:
        assert_eq!(remap_index_after_reorder(0, 1, 3), 0);
        // Shifted left inside range (1, 3]:
        assert_eq!(remap_index_after_reorder(2, 1, 3), 1);
        assert_eq!(remap_index_after_reorder(3, 1, 3), 2);
        // Untouched right of to:
        assert_eq!(remap_index_after_reorder(4, 1, 3), 4);

        // Moving left: from 3 to 1
        // Target:
        assert_eq!(remap_index_after_reorder(3, 3, 1), 1);
        // Untouched left of to:
        assert_eq!(remap_index_after_reorder(0, 3, 1), 0);
        // Shifted right inside range [1, 3):
        assert_eq!(remap_index_after_reorder(1, 3, 1), 2);
        assert_eq!(remap_index_after_reorder(2, 3, 1), 3);
        // Untouched right of from:
        assert_eq!(remap_index_after_reorder(4, 3, 1), 4);

        // No-op (from == to):
        assert_eq!(remap_index_after_reorder(2, 2, 2), 2);
        assert_eq!(remap_index_after_reorder(0, 2, 2), 0);
    }
}
