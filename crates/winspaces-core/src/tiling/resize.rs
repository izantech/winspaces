//! Drag gesture classification for mouse drag-swap and border drag-resize.

use windows_sys::Win32::Foundation::HWND;
use winspaces_common::WindowRect;

use super::types::{DragOutcome, MAX_RATIO, MIN_RATIO};

/// Classify a window drag gesture after `MOVESIZEEND` on a tiled space.
///
/// Distinguishes between:
/// 1. Primary split ratio adjustment (border drag on split 0).
/// 2. Shift-held positional drag -> primary split orientation toggle.
/// 3. Drag-swap reorder (dragging a window and dropping it over another tile's area).
/// 4. Snap-back (deep split border drag, ambiguous motion, or dropping outside tiles).
///
/// `shift_held` only reroutes *move* gestures: border drags keep their ratio
/// semantics with or without the modifier. `from_maximized` marks a drag that
/// began on a maximized window: Windows restores it on the first move, so the
/// size delta is the restore, never a border resize.
pub fn classify_drag(
    dragged_hwnd: HWND,
    old_rect: &WindowRect,
    new_rect: &WindowRect,
    tiles: &[(HWND, WindowRect)],
    drop_point: (i32, i32),
    shift_held: bool,
    from_maximized: bool,
) -> DragOutcome {
    let from_idx = tiles.iter().position(|(h, _)| *h == dragged_hwnd);
    let Some(from) = from_idx else {
        return DragOutcome::SnapBack;
    };

    let dw = (new_rect.width() - old_rect.width()).abs();
    let dh = (new_rect.height() - old_rect.height()).abs();

    // 1. Size change -> ratio resize (a restore-on-drag is not a resize)
    if (dw > 4 || dh > 4) && !from_maximized {
        if tiles.len() >= 2 && (from == 0 || from == 1) {
            let t0 = &tiles[0].1;
            let t1 = &tiles[1].1;

            // Check if split 0 is horizontal (side-by-side)
            if (t0.top - t1.top).abs() <= 4 && (t0.bottom - t1.bottom).abs() <= 4 {
                let total_w = (t0.width() + t1.width()) as f32;
                if total_w > 0.0 {
                    if from == 0 {
                        let new_w = new_rect.width() as f32;
                        let ratio = (new_w / total_w).clamp(MIN_RATIO, MAX_RATIO);
                        return DragOutcome::AdjustDwindleRatio {
                            ratio_index: 0,
                            new_ratio: ratio,
                        };
                    } else if from == 1 {
                        let new_w = new_rect.width() as f32;
                        let ratio = ((total_w - new_w) / total_w).clamp(MIN_RATIO, MAX_RATIO);
                        return DragOutcome::AdjustDwindleRatio {
                            ratio_index: 0,
                            new_ratio: ratio,
                        };
                    }
                }
            }

            // Check if split 0 is vertical (stacked)
            if (t0.left - t1.left).abs() <= 4 && (t0.right - t1.right).abs() <= 4 {
                let total_h = (t0.height() + t1.height()) as f32;
                if total_h > 0.0 {
                    if from == 0 {
                        let new_h = new_rect.height() as f32;
                        let ratio = (new_h / total_h).clamp(MIN_RATIO, MAX_RATIO);
                        return DragOutcome::AdjustDwindleRatio {
                            ratio_index: 0,
                            new_ratio: ratio,
                        };
                    } else if from == 1 {
                        let new_h = new_rect.height() as f32;
                        let ratio = ((total_h - new_h) / total_h).clamp(MIN_RATIO, MAX_RATIO);
                        return DragOutcome::AdjustDwindleRatio {
                            ratio_index: 0,
                            new_ratio: ratio,
                        };
                    }
                }
            }
        }

        // Deeper splits or non-matching border drags snap back
        return DragOutcome::SnapBack;
    }

    // 2. Shift-held move -> split orientation toggle
    if shift_held {
        return DragOutcome::ToggleSplit;
    }

    // 3. Position change -> check drop target
    let to_idx = tiles.iter().position(|(_, r)| {
        drop_point.0 >= r.left
            && drop_point.0 < r.right
            && drop_point.1 >= r.top
            && drop_point.1 < r.bottom
    });

    if let Some(to) = to_idx {
        if from != to {
            return DragOutcome::Reorder {
                from_index: from,
                to_index: to,
            };
        }
    }

    DragOutcome::SnapBack
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fake_hwnd(val: usize) -> HWND {
        val as *mut std::ffi::c_void
    }

    #[test]
    fn horizontal_split_0_ratio_drag_adjusts_ratio() {
        let w0 = fake_hwnd(1);
        let w1 = fake_hwnd(2);

        let t0 = WindowRect {
            left: 0,
            top: 0,
            right: 960,
            bottom: 1040,
        };
        let t1 = WindowRect {
            left: 960,
            top: 0,
            right: 1920,
            bottom: 1040,
        };
        let tiles = [(w0, t0.clone()), (w1, t1)];

        // User dragged t0's right border to 1200px
        let new_rect = WindowRect {
            left: 0,
            top: 0,
            right: 1200,
            bottom: 1040,
        };
        let outcome = classify_drag(w0, &t0, &new_rect, &tiles, (1200, 500), false, false);

        let expected_ratio = (1200.0 / 1920.0f32).clamp(MIN_RATIO, MAX_RATIO);
        assert_eq!(
            outcome,
            DragOutcome::AdjustDwindleRatio {
                ratio_index: 0,
                new_ratio: expected_ratio
            }
        );
    }

    #[test]
    fn drag_onto_another_tile_swaps() {
        let w0 = fake_hwnd(1);
        let w1 = fake_hwnd(2);

        let t0 = WindowRect {
            left: 0,
            top: 0,
            right: 960,
            bottom: 1040,
        };
        let t1 = WindowRect {
            left: 960,
            top: 0,
            right: 1920,
            bottom: 1040,
        };
        let tiles = [(w0, t0.clone()), (w1, t1.clone())];

        // User moved w0 without changing size, dropping cursor at (1400, 500) inside t1
        let moved_rect = WindowRect {
            left: 500,
            top: 100,
            right: 1460,
            bottom: 1140,
        };
        let outcome = classify_drag(w0, &t0, &moved_rect, &tiles, (1400, 500), false, false);

        assert_eq!(
            outcome,
            DragOutcome::Reorder {
                from_index: 0,
                to_index: 1
            }
        );
    }

    #[test]
    fn drop_outside_or_same_tile_snaps_back() {
        let w0 = fake_hwnd(1);
        let w1 = fake_hwnd(2);

        let t0 = WindowRect {
            left: 0,
            top: 0,
            right: 960,
            bottom: 1040,
        };
        let t1 = WindowRect {
            left: 960,
            top: 0,
            right: 1920,
            bottom: 1040,
        };
        let tiles = [(w0, t0.clone()), (w1, t1)];

        // Dropped outside work area
        let moved_rect = WindowRect {
            left: 100,
            top: 100,
            right: 1060,
            bottom: 1140,
        };
        let outcome_outside =
            classify_drag(w0, &t0, &moved_rect, &tiles, (2500, 500), false, false);
        assert_eq!(outcome_outside, DragOutcome::SnapBack);

        // Dropped inside own tile
        let outcome_own = classify_drag(w0, &t0, &moved_rect, &tiles, (400, 500), false, false);
        assert_eq!(outcome_own, DragOutcome::SnapBack);
    }

    #[test]
    fn shift_move_toggles_split_regardless_of_drop_point() {
        let w0 = fake_hwnd(1);
        let w1 = fake_hwnd(2);

        let t0 = WindowRect {
            left: 0,
            top: 0,
            right: 960,
            bottom: 1040,
        };
        let t1 = WindowRect {
            left: 960,
            top: 0,
            right: 1920,
            bottom: 1040,
        };
        let tiles = [(w0, t0.clone()), (w1, t1)];

        // Position-only drag (no size change)
        let moved_rect = WindowRect {
            left: 500,
            top: 100,
            right: 1460,
            bottom: 1140,
        };

        // Dropped over another tile with Shift
        let over_other = classify_drag(w0, &t0, &moved_rect, &tiles, (1400, 500), true, false);
        assert_eq!(over_other, DragOutcome::ToggleSplit);

        // Dropped inside own tile with Shift
        let over_own = classify_drag(w0, &t0, &moved_rect, &tiles, (400, 500), true, false);
        assert_eq!(over_own, DragOutcome::ToggleSplit);

        // Dropped outside all tiles with Shift
        let outside = classify_drag(w0, &t0, &moved_rect, &tiles, (2500, 500), true, false);
        assert_eq!(outside, DragOutcome::ToggleSplit);
    }

    #[test]
    fn drag_from_maximized_ignores_restore_size_delta() {
        let w0 = fake_hwnd(1);
        let w1 = fake_hwnd(2);

        let t0 = WindowRect {
            left: 0,
            top: 0,
            right: 960,
            bottom: 1040,
        };
        let t1 = WindowRect {
            left: 960,
            top: 0,
            right: 1920,
            bottom: 1040,
        };
        let tiles = [(w0, t0.clone()), (w1, t1)];

        // The drag began on the maximized frame (whole work area); Windows
        // restored it mid-drag to a rect that is neither the tile nor a
        // border adjustment.
        let maximized = WindowRect {
            left: 0,
            top: 0,
            right: 1920,
            bottom: 1040,
        };
        let restored = WindowRect {
            left: 500,
            top: 200,
            right: 1300,
            bottom: 900,
        };

        // Dropped over t1 -> reorder, not a ratio change.
        let over_other = classify_drag(w0, &maximized, &restored, &tiles, (1400, 500), false, true);
        assert_eq!(
            over_other,
            DragOutcome::Reorder {
                from_index: 0,
                to_index: 1
            }
        );

        // Dropped over its own slot -> snap back into it.
        let over_own = classify_drag(w0, &maximized, &restored, &tiles, (400, 500), false, true);
        assert_eq!(over_own, DragOutcome::SnapBack);

        // Shift still means split toggle.
        let shifted = classify_drag(w0, &maximized, &restored, &tiles, (400, 500), true, true);
        assert_eq!(shifted, DragOutcome::ToggleSplit);
    }
}
