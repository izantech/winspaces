//! Pure layout algorithms for dynamic tiling (Dwindle and MasterStack).

use winspaces_common::WindowRect;

use super::types::{Gaps, LayoutKind, DEFAULT_RATIO, MAX_RATIO, MIN_RATIO};

/// Compute tile bounding rectangles for `n` windows within `work` area.
pub fn compute(
    layout: LayoutKind,
    work: &WindowRect,
    n: usize,
    ratios: &[f32],
    gaps: &Gaps,
) -> Vec<WindowRect> {
    match layout {
        LayoutKind::Dwindle => compute_dwindle(work, n, ratios, gaps),
        LayoutKind::MasterStack => compute_master_stack(work, n, ratios, gaps),
    }
}

/// Compute dwindle (BSP) tile positions.
///
/// In dwindle layout, each successive window splits the remaining bounding box
/// along its longer dimension according to the configured split ratio.
pub fn compute_dwindle(
    work: &WindowRect,
    n: usize,
    ratios: &[f32],
    gaps: &Gaps,
) -> Vec<WindowRect> {
    if n == 0 {
        return Vec::new();
    }

    let og = gaps.outer as i32;
    let mut current_rect = WindowRect {
        left: work.left + og,
        top: work.top + og,
        right: (work.right - og).max(work.left + og),
        bottom: (work.bottom - og).max(work.top + og),
    };

    if n == 1 {
        return vec![current_rect];
    }

    let mut tiles = Vec::with_capacity(n);
    let ig = gaps.inner as i32;

    for i in 0..(n - 1) {
        let w = current_rect.width();
        let h = current_rect.height();
        let ratio = ratios
            .get(i)
            .copied()
            .unwrap_or(DEFAULT_RATIO)
            .clamp(MIN_RATIO, MAX_RATIO);

        if w >= h {
            // Split horizontally (vertical separator)
            let available_w = (w - ig).max(0);
            let w_first = ((available_w as f32) * ratio).round() as i32;

            let first_tile = WindowRect {
                left: current_rect.left,
                top: current_rect.top,
                right: current_rect.left + w_first,
                bottom: current_rect.bottom,
            };
            tiles.push(first_tile);

            current_rect = WindowRect {
                left: current_rect.left + w_first + ig,
                top: current_rect.top,
                right: current_rect.right,
                bottom: current_rect.bottom,
            };
        } else {
            // Split vertically (horizontal separator)
            let available_h = (h - ig).max(0);
            let h_first = ((available_h as f32) * ratio).round() as i32;

            let first_tile = WindowRect {
                left: current_rect.left,
                top: current_rect.top,
                right: current_rect.right,
                bottom: current_rect.top + h_first,
            };
            tiles.push(first_tile);

            current_rect = WindowRect {
                left: current_rect.left,
                top: current_rect.top + h_first + ig,
                right: current_rect.right,
                bottom: current_rect.bottom,
            };
        }
    }

    tiles.push(current_rect);
    tiles
}

/// Compute master-stack tile positions (master on left, stack on right).
pub fn compute_master_stack(
    work: &WindowRect,
    n: usize,
    ratios: &[f32],
    gaps: &Gaps,
) -> Vec<WindowRect> {
    if n == 0 {
        return Vec::new();
    }

    let og = gaps.outer as i32;
    let inset_work = WindowRect {
        left: work.left + og,
        top: work.top + og,
        right: (work.right - og).max(work.left + og),
        bottom: (work.bottom - og).max(work.top + og),
    };

    if n == 1 {
        return vec![inset_work];
    }

    let ig = gaps.inner as i32;
    let w = inset_work.width();
    let ratio = ratios
        .first()
        .copied()
        .unwrap_or(DEFAULT_RATIO)
        .clamp(MIN_RATIO, MAX_RATIO);

    let available_w = (w - ig).max(0);
    let master_w = ((available_w as f32) * ratio).round() as i32;

    let mut tiles = Vec::with_capacity(n);
    tiles.push(WindowRect {
        left: inset_work.left,
        top: inset_work.top,
        right: inset_work.left + master_w,
        bottom: inset_work.bottom,
    });

    let stack_left = inset_work.left + master_w + ig;
    let stack_right = inset_work.right;
    let stack_count = (n - 1) as i32;
    let total_h = inset_work.height();
    let total_inner_gaps = (stack_count - 1) * ig;
    let available_h = (total_h - total_inner_gaps).max(0);

    let mut curr_y = inset_work.top;
    for i in 0..(n - 1) {
        let slot_h = if i == (n - 2) {
            inset_work.bottom - curr_y
        } else {
            available_h / stack_count
        };

        tiles.push(WindowRect {
            left: stack_left,
            top: curr_y,
            right: stack_right,
            bottom: curr_y + slot_h,
        });
        curr_y += slot_h + ig;
    }

    tiles
}

#[cfg(test)]
mod tests {
    use super::*;

    fn work_area_standard() -> WindowRect {
        WindowRect {
            left: 0,
            top: 0,
            right: 1920,
            bottom: 1040,
        }
    }

    fn work_area_negative() -> WindowRect {
        WindowRect {
            left: -1920,
            top: 0,
            right: 0,
            bottom: 1040,
        }
    }

    #[test]
    fn dwindle_empty_returns_empty() {
        let res = compute_dwindle(&work_area_standard(), 0, &[], &Gaps::NONE);
        assert!(res.is_empty());
    }

    #[test]
    fn dwindle_single_window_fills_work_area() {
        let work = work_area_standard();
        let tiles = compute_dwindle(&work, 1, &[], &Gaps::NONE);
        assert_eq!(tiles.len(), 1);
        assert_eq!(tiles[0], work);
    }

    #[test]
    fn dwindle_two_windows_splits_longer_side_half() {
        let work = work_area_standard();
        let tiles = compute_dwindle(&work, 2, &[], &Gaps::NONE);
        assert_eq!(tiles.len(), 2);
        // 1920 >= 1040, so horizontal split at 960
        assert_eq!(
            tiles[0],
            WindowRect {
                left: 0,
                top: 0,
                right: 960,
                bottom: 1040
            }
        );
        assert_eq!(
            tiles[1],
            WindowRect {
                left: 960,
                top: 0,
                right: 1920,
                bottom: 1040
            }
        );
    }

    #[test]
    fn dwindle_three_windows_spirals() {
        let work = work_area_standard();
        let tiles = compute_dwindle(&work, 3, &[], &Gaps::NONE);
        assert_eq!(tiles.len(), 3);
        // Window 0: Left half (960x1040)
        assert_eq!(
            tiles[0],
            WindowRect {
                left: 0,
                top: 0,
                right: 960,
                bottom: 1040
            }
        );
        // Remaining right region is 960x1040 (h > w, 1040 > 960) -> splits vertically at y=520
        // Window 1: Top-right (960x520)
        assert_eq!(
            tiles[1],
            WindowRect {
                left: 960,
                top: 0,
                right: 1920,
                bottom: 520
            }
        );
        // Window 2: Bottom-right (960x520)
        assert_eq!(
            tiles[2],
            WindowRect {
                left: 960,
                top: 520,
                right: 1920,
                bottom: 1040
            }
        );
    }

    #[test]
    fn dwindle_ratio_custom_and_clamped() {
        let work = work_area_standard();
        // Ratio 0.7 for first split
        let tiles = compute_dwindle(&work, 2, &[0.7], &Gaps::NONE);
        let expected_w0 = (1920.0 * 0.7f32).round() as i32;
        assert_eq!(tiles[0].width(), expected_w0);
        assert_eq!(tiles[1].width(), 1920 - expected_w0);

        // Ratio > 0.9 is clamped to 0.9
        let tiles_clamped = compute_dwindle(&work, 2, &[0.99], &Gaps::NONE);
        let expected_w_clamped = (1920.0 * 0.9f32).round() as i32;
        assert_eq!(tiles_clamped[0].width(), expected_w_clamped);
    }

    #[test]
    fn dwindle_outer_and_inner_gaps() {
        let work = work_area_standard();
        let gaps = Gaps::new(10, 20);
        let tiles = compute_dwindle(&work, 2, &[], &gaps);
        assert_eq!(tiles.len(), 2);
        // Inset work: left=20, top=20, right=1900, bottom=1020, width=1880
        // Available w = 1880 - 10 = 1870 -> 50% = 935
        assert_eq!(
            tiles[0],
            WindowRect {
                left: 20,
                top: 20,
                right: 20 + 935,
                bottom: 1020
            }
        );
        assert_eq!(
            tiles[1],
            WindowRect {
                left: 20 + 935 + 10,
                top: 20,
                right: 1900,
                bottom: 1020
            }
        );
    }

    fn rect_area(r: &WindowRect) -> i64 {
        (r.width().max(0) as i64) * (r.height().max(0) as i64)
    }

    fn rects_overlap(a: &WindowRect, b: &WindowRect) -> bool {
        let x_overlap = (a.right.min(b.right) - a.left.max(b.left)).max(0);
        let y_overlap = (a.bottom.min(b.bottom) - a.top.max(b.top)).max(0);
        x_overlap > 0 && y_overlap > 0
    }

    #[test]
    fn exactness_invariant_standard_and_negative_work_areas() {
        for work in [work_area_standard(), work_area_negative()] {
            let total_work_area = rect_area(&work);

            for n in 1..=6 {
                let tiles = compute_dwindle(&work, n, &[], &Gaps::NONE);
                assert_eq!(tiles.len(), n);

                // 1. Total area sum equals work area exactly
                let sum_area: i64 = tiles.iter().map(rect_area).sum();
                assert_eq!(
                    sum_area, total_work_area,
                    "Area mismatch for n={n} on work={work:?}"
                );

                // 2. No two tiles overlap
                for i in 0..n {
                    for j in (i + 1)..n {
                        assert!(
                            !rects_overlap(&tiles[i], &tiles[j]),
                            "Tiles {i} and {j} overlap for n={n}: {:?} vs {:?}",
                            tiles[i],
                            tiles[j]
                        );
                    }
                }

                // 3. Every tile is within work bounds
                for (idx, tile) in tiles.iter().enumerate() {
                    assert!(tile.left >= work.left, "tile {idx} left < work left");
                    assert!(tile.top >= work.top, "tile {idx} top < work top");
                    assert!(tile.right <= work.right, "tile {idx} right > work right");
                    assert!(
                        tile.bottom <= work.bottom,
                        "tile {idx} bottom > work bottom"
                    );
                }
            }
        }
    }
}
