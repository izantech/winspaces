//! Directional spatial navigation between tiled windows.

use windows_sys::Win32::Foundation::HWND;
use winspaces_common::WindowRect;

use super::types::Direction;

/// Find the neighboring tiled window in the given spatial direction.
///
/// Returns `None` if `target` is not found in `tiles` or no candidate window exists
/// in that direction.
pub fn directional_neighbor(
    target: HWND,
    dir: Direction,
    tiles: &[(HWND, WindowRect)],
) -> Option<HWND> {
    let target_rect = tiles.iter().find(|(h, _)| *h == target).map(|(_, r)| r)?;

    let mut best: Option<(HWND, i32, i32, usize)> = None; // (hwnd, dist, neg_overlap, index)

    for (idx, &(cand_hwnd, ref cand_rect)) in tiles.iter().enumerate() {
        if cand_hwnd == target {
            continue;
        }

        let (dist, overlap) = match dir {
            Direction::Left => {
                if cand_rect.right <= target_rect.left {
                    let d = target_rect.left - cand_rect.right;
                    let ov = (cand_rect.bottom.min(target_rect.bottom)
                        - cand_rect.top.max(target_rect.top))
                    .max(0);
                    (d, ov)
                } else {
                    continue;
                }
            }
            Direction::Right => {
                if cand_rect.left >= target_rect.right {
                    let d = cand_rect.left - target_rect.right;
                    let ov = (cand_rect.bottom.min(target_rect.bottom)
                        - cand_rect.top.max(target_rect.top))
                    .max(0);
                    (d, ov)
                } else {
                    continue;
                }
            }
            Direction::Up => {
                if cand_rect.bottom <= target_rect.top {
                    let d = target_rect.top - cand_rect.bottom;
                    let ov = (cand_rect.right.min(target_rect.right)
                        - cand_rect.left.max(target_rect.left))
                    .max(0);
                    (d, ov)
                } else {
                    continue;
                }
            }
            Direction::Down => {
                if cand_rect.top >= target_rect.bottom {
                    let d = cand_rect.top - target_rect.bottom;
                    let ov = (cand_rect.right.min(target_rect.right)
                        - cand_rect.left.max(target_rect.left))
                    .max(0);
                    (d, ov)
                } else {
                    continue;
                }
            }
        };

        // Prefer: 1) minimum distance, 2) maximum overlap (more negative neg_overlap), 3) earlier index
        let score = (dist, -overlap, idx);
        match &best {
            None => best = Some((cand_hwnd, score.0, score.1, score.2)),
            Some((_, b_dist, b_neg_ov, b_idx)) => {
                let b_score = (*b_dist, *b_neg_ov, *b_idx);
                if score < b_score {
                    best = Some((cand_hwnd, score.0, score.1, score.2));
                }
            }
        }
    }

    best.map(|(hwnd, _, _, _)| hwnd)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fake_hwnd(val: usize) -> HWND {
        val as *mut std::ffi::c_void
    }

    #[test]
    fn grid_2x2_navigation() {
        let tl = fake_hwnd(1);
        let tr = fake_hwnd(2);
        let bl = fake_hwnd(3);
        let br = fake_hwnd(4);

        let tiles = [
            (
                tl,
                WindowRect {
                    left: 0,
                    top: 0,
                    right: 960,
                    bottom: 520,
                },
            ),
            (
                tr,
                WindowRect {
                    left: 960,
                    top: 0,
                    right: 1920,
                    bottom: 520,
                },
            ),
            (
                bl,
                WindowRect {
                    left: 0,
                    top: 520,
                    right: 960,
                    bottom: 1040,
                },
            ),
            (
                br,
                WindowRect {
                    left: 960,
                    top: 520,
                    right: 1920,
                    bottom: 1040,
                },
            ),
        ];

        // Top-left tests
        assert_eq!(directional_neighbor(tl, Direction::Right, &tiles), Some(tr));
        assert_eq!(directional_neighbor(tl, Direction::Down, &tiles), Some(bl));
        assert_eq!(directional_neighbor(tl, Direction::Left, &tiles), None);
        assert_eq!(directional_neighbor(tl, Direction::Up, &tiles), None);

        // Bottom-right tests
        assert_eq!(directional_neighbor(br, Direction::Left, &tiles), Some(bl));
        assert_eq!(directional_neighbor(br, Direction::Up, &tiles), Some(tr));
        assert_eq!(directional_neighbor(br, Direction::Right, &tiles), None);
        assert_eq!(directional_neighbor(br, Direction::Down, &tiles), None);
    }

    #[test]
    fn spiral_3_window_overlap_preference() {
        let left_w = fake_hwnd(1);
        let tr_w = fake_hwnd(2);
        let br_w = fake_hwnd(3);

        let tiles = [
            (
                left_w,
                WindowRect {
                    left: 0,
                    top: 0,
                    right: 960,
                    bottom: 1040,
                },
            ),
            (
                tr_w,
                WindowRect {
                    left: 960,
                    top: 0,
                    right: 1920,
                    bottom: 520,
                },
            ),
            (
                br_w,
                WindowRect {
                    left: 960,
                    top: 520,
                    right: 1920,
                    bottom: 1040,
                },
            ),
        ];

        // From TR window, moving left hits left_w
        assert_eq!(
            directional_neighbor(tr_w, Direction::Left, &tiles),
            Some(left_w)
        );
        // From BR window, moving left hits left_w
        assert_eq!(
            directional_neighbor(br_w, Direction::Left, &tiles),
            Some(left_w)
        );
        // From TR window, moving down hits br_w
        assert_eq!(
            directional_neighbor(tr_w, Direction::Down, &tiles),
            Some(br_w)
        );
        // From BR window, moving up hits tr_w
        assert_eq!(
            directional_neighbor(br_w, Direction::Up, &tiles),
            Some(tr_w)
        );
    }
}
