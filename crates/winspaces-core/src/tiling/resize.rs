//! Drag gesture classification for mouse drag-swap and border drag-resize.

use windows_sys::Win32::Foundation::HWND;
use winspaces_common::WindowRect;

use super::types::{DragOutcome, MAX_RATIO, MIN_RATIO};

/// Classify a window drag gesture after `MOVESIZEEND` on a tiled space.
///
/// Distinguishes between:
/// 1. Primary split ratio adjustment (border drag on split 0).
/// 2. Drag-swap reorder (dragging a window and dropping it over another tile's area).
/// 3. Snap-back (deep split border drag, ambiguous motion, or dropping outside tiles).
pub fn classify_drag(
    old_rect: &WindowRect,
    new_rect: &WindowRect,
    tiles: &[(HWND, WindowRect)],
    drop_point: (i32, i32),
) -> DragOutcome {
    let dw = (new_rect.width() - old_rect.width()).abs();
    let dh = (new_rect.height() - old_rect.height()).abs();

    // 1. Size change -> ratio resize
    if dw > 4 || dh > 4 {
        if tiles.len() >= 2 {
            let t0 = &tiles[0].1;
            let t1 = &tiles[1].1;

            // Check if split 0 is horizontal (side-by-side)
            if t0.top == t1.top && t0.bottom == t1.bottom {
                let total_w = (t0.width() + t1.width()) as f32;
                if total_w > 0.0 {
                    // Check if dragged window is t0
                    if *old_rect == *t0 {
                        let new_w = new_rect.width() as f32;
                        let ratio = (new_w / total_w).clamp(MIN_RATIO, MAX_RATIO);
                        return DragOutcome::AdjustDwindleRatio {
                            ratio_index: 0,
                            new_ratio: ratio,
                        };
                    }
                    // Or dragged window is t1
                    if *old_rect == *t1 {
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
            if t0.left == t1.left && t0.right == t1.right {
                let total_h = (t0.height() + t1.height()) as f32;
                if total_h > 0.0 {
                    if *old_rect == *t0 {
                        let new_h = new_rect.height() as f32;
                        let ratio = (new_h / total_h).clamp(MIN_RATIO, MAX_RATIO);
                        return DragOutcome::AdjustDwindleRatio {
                            ratio_index: 0,
                            new_ratio: ratio,
                        };
                    }
                    if *old_rect == *t1 {
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

        // Deeper splits or non-matching border drags snap back in v1
        return DragOutcome::SnapBack;
    }

    // 2. Position change -> check drop target
    let from_idx = tiles.iter().position(|(_, r)| *r == *old_rect);
    let to_idx = tiles.iter().position(|(_, r)| {
        drop_point.0 >= r.left
            && drop_point.0 < r.right
            && drop_point.1 >= r.top
            && drop_point.1 < r.bottom
    });

    if let (Some(from), Some(to)) = (from_idx, to_idx) {
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
        let outcome = classify_drag(&t0, &new_rect, &tiles, (1200, 500));

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
        let outcome = classify_drag(&t0, &moved_rect, &tiles, (1400, 500));

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
        let outcome_outside = classify_drag(&t0, &moved_rect, &tiles, (2500, 500));
        assert_eq!(outcome_outside, DragOutcome::SnapBack);

        // Dropped inside own tile
        let outcome_own = classify_drag(&t0, &moved_rect, &tiles, (400, 500));
        assert_eq!(outcome_own, DragOutcome::SnapBack);
    }
}
