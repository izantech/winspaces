//! Window membership and stable slot ordering reconciliation.

use std::collections::HashSet;
use windows_sys::Win32::Foundation::HWND;

/// Reconcile the current slot order with live candidate windows on a space.
///
/// - Retains the relative ordering of surviving windows already in `current_order`.
/// - Drops dead / untracked / floated windows (not present in `candidates`).
/// - Inserts newly discovered candidate windows immediately after the currently `focused` window
///   (if focused is in the retained order), or appends them to the end.
pub fn reconcile_order(
    current_order: &[HWND],
    candidates: &[HWND],
    focused: Option<HWND>,
) -> Vec<HWND> {
    let candidate_set: HashSet<HWND> = candidates.iter().copied().collect();

    // 1. Retain surviving windows from current_order
    let mut order: Vec<HWND> = current_order
        .iter()
        .copied()
        .filter(|h| candidate_set.contains(h))
        .collect();

    let existing_set: HashSet<HWND> = order.iter().copied().collect();

    // 2. Identify new candidates not yet in order
    let new_candidates: Vec<HWND> = candidates
        .iter()
        .copied()
        .filter(|h| !existing_set.contains(h))
        .collect();

    if new_candidates.is_empty() {
        return order;
    }

    // 3. Find insertion position based on focused window
    let mut insert_pos = match focused {
        Some(f) => order.iter().position(|&h| h == f).map(|pos| pos + 1),
        None => None,
    };

    for new_hwnd in new_candidates {
        if let Some(pos) = insert_pos {
            order.insert(pos, new_hwnd);
            insert_pos = Some(pos + 1);
        } else {
            order.push(new_hwnd);
        }
    }

    order
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fake_hwnd(val: usize) -> HWND {
        val as *mut std::ffi::c_void
    }

    #[test]
    fn order_retains_survivors_and_drops_dead() {
        let w1 = fake_hwnd(1);
        let w2 = fake_hwnd(2);
        let w3 = fake_hwnd(3);

        let current = [w1, w2, w3];
        // w2 died
        let candidates = [w3, w1];

        let result = reconcile_order(&current, &candidates, None);
        assert_eq!(result, vec![w1, w3]);
    }

    #[test]
    fn insert_after_focused_window() {
        let w1 = fake_hwnd(1);
        let w2 = fake_hwnd(2);
        let w_new = fake_hwnd(99);

        let current = [w1, w2];
        let candidates = [w1, w2, w_new];

        // w1 is focused -> w_new should be inserted at index 1 (after w1)
        let result = reconcile_order(&current, &candidates, Some(w1));
        assert_eq!(result, vec![w1, w_new, w2]);
    }

    #[test]
    fn insert_multiple_after_focused_window() {
        let w1 = fake_hwnd(1);
        let w2 = fake_hwnd(2);
        let w_new1 = fake_hwnd(91);
        let w_new2 = fake_hwnd(92);

        let current = [w1, w2];
        let candidates = [w1, w2, w_new1, w_new2];

        let result = reconcile_order(&current, &candidates, Some(w1));
        assert_eq!(result, vec![w1, w_new1, w_new2, w2]);
    }

    #[test]
    fn append_when_no_focus_or_unmatched_focus() {
        let w1 = fake_hwnd(1);
        let w2 = fake_hwnd(2);
        let w_new = fake_hwnd(99);

        let current = [w1, w2];
        let candidates = [w1, w2, w_new];

        // No focused window
        let result = reconcile_order(&current, &candidates, None);
        assert_eq!(result, vec![w1, w2, w_new]);

        // Focused window is not in current order
        let result_unmatched = reconcile_order(&current, &candidates, Some(fake_hwnd(555)));
        assert_eq!(result_unmatched, vec![w1, w2, w_new]);
    }

    #[test]
    fn idempotent_when_candidates_unchanged() {
        let w1 = fake_hwnd(1);
        let w2 = fake_hwnd(2);
        let w3 = fake_hwnd(3);

        let current = [w1, w2, w3];
        let candidates = [w1, w2, w3];

        let result = reconcile_order(&current, &candidates, Some(w2));
        assert_eq!(result, vec![w1, w2, w3]);
    }
}
