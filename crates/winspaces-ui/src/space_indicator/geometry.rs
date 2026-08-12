//! Pure layout and mask math for the space indicator: no `HWND`, no Win32
//! call, nothing but rects and numbers in, rects and coverages out — the same
//! testability model as `mission_control::geometry`.

use windows_sys::Win32::Foundation::RECT;
use winspaces_win32::dpi::px;

/// Panel height at 96 dpi.
pub const HEIGHT: i32 = 60;
/// Horizontal padding either side of the label at 96 dpi.
pub const PAD_X: i32 = 32;
/// Floor on panel width at 96 dpi, so "Space 1" and "Space 9" render the same
/// size instead of the panel breathing with the glyph.
pub const MIN_WIDTH: i32 = 168;
/// Gap between the panel's bottom edge and the work area's, at 96 dpi.
pub const GAP: i32 = 16;
/// Corner radius at 96 dpi — the Windows 11 flyout radius.
pub const RADIUS: i32 = 10;
/// Font cell height at 96 dpi (passed to `create_font` negated).
pub const FONT_HEIGHT: i32 = 20;

/// Where the panel sits: horizontally centered on `work`, resting `GAP` above
/// its bottom edge.
///
/// `work` is the taskbar-excluded rect, so this lands just above the taskbar
/// on a bottom-docked setup and just above the screen edge on a top- or
/// side-docked one, with no per-orientation special casing.
pub fn indicator_rect(work: RECT, scale: f32, text_w: i32) -> RECT {
    let work_w = (work.right - work.left).max(1);
    let work_h = (work.bottom - work.top).max(1);

    let width = (text_w + 2 * px(scale, PAD_X))
        .max(px(scale, MIN_WIDTH))
        .min(work_w);
    let height = px(scale, HEIGHT).min(work_h);

    let left = work.left + (work_w - width) / 2;
    let bottom = (work.bottom - px(scale, GAP)).max(work.top + height);
    let top = bottom - height;

    RECT {
        left,
        top,
        right: left + width,
        bottom: top + height,
    }
}

/// Anti-aliased coverage of pixel `(x, y)` inside a rounded rectangle of size
/// `w` x `h`, shrunk by `inset` on every side, in `0.0..=1.0`.
///
/// This is what rounds the panel: GDI has no anti-aliased `RoundRect`, and the
/// tray badge's equivalent mask (`tray/badge.rs`) is a hard in/out test that
/// leaves visibly stepped corners — acceptable on a 16 px icon, not on a 60 px
/// panel. Signed-distance instead, so the corners land soft.
pub fn round_rect_coverage(x: i32, y: i32, w: i32, h: i32, radius: f32, inset: f32) -> f32 {
    // Pixel center relative to the rect center.
    let px_ = (x as f32 + 0.5) - w as f32 / 2.0;
    let py = (y as f32 + 0.5) - h as f32 / 2.0;

    let half_w = w as f32 / 2.0 - inset;
    let half_h = h as f32 / 2.0 - inset;
    if half_w <= 0.0 || half_h <= 0.0 {
        return 0.0;
    }
    let r = radius.min(half_w).min(half_h).max(0.0);

    // Distance to the rounded box, negative inside.
    let qx = px_.abs() - (half_w - r);
    let qy = py.abs() - (half_h - r);
    let outside = (qx.max(0.0).powi(2) + qy.max(0.0).powi(2)).sqrt();
    let inside = qx.max(qy).min(0.0);
    let dist = outside + inside - r;

    // One-pixel-wide ramp centered on the boundary.
    (0.5 - dist).clamp(0.0, 1.0)
}

/// Smoothstep easing on `0.0..=1.0`, so the fade eases in and out instead of
/// ramping linearly.
pub fn smoothstep(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn work() -> RECT {
        RECT {
            left: 0,
            top: 0,
            right: 1920,
            bottom: 1032, // 1080 minus a 48 px taskbar
        }
    }

    #[test]
    fn panel_is_horizontally_centered() {
        let r = indicator_rect(work(), 1.0, 80);
        let width = r.right - r.left;
        assert_eq!(r.left, (1920 - width) / 2);
        assert_eq!(1920 - r.right, r.left);
    }

    #[test]
    fn panel_rests_above_the_work_area_bottom() {
        let r = indicator_rect(work(), 1.0, 80);
        assert_eq!(r.bottom, 1032 - GAP);
        assert_eq!(r.bottom - r.top, HEIGHT);
    }

    #[test]
    fn narrow_labels_hit_the_width_floor() {
        let r = indicator_rect(work(), 1.0, 10);
        assert_eq!(r.right - r.left, MIN_WIDTH);
    }

    #[test]
    fn wide_labels_grow_past_the_floor() {
        let r = indicator_rect(work(), 1.0, 400);
        assert_eq!(r.right - r.left, 400 + 2 * PAD_X);
    }

    #[test]
    fn scale_applies_to_every_metric() {
        let r = indicator_rect(work(), 2.0, 10);
        assert_eq!(r.right - r.left, MIN_WIDTH * 2);
        assert_eq!(r.bottom - r.top, HEIGHT * 2);
        assert_eq!(r.bottom, 1032 - GAP * 2);
    }

    #[test]
    fn panel_never_exceeds_a_narrow_work_area() {
        let narrow = RECT {
            left: 100,
            top: 0,
            right: 220,
            bottom: 300,
        };
        let r = indicator_rect(narrow, 1.0, 400);
        assert_eq!(r.left, 100);
        assert_eq!(r.right, 220);
    }

    #[test]
    fn offset_monitor_keeps_its_own_origin() {
        let secondary = RECT {
            left: -1920,
            top: 0,
            right: 0,
            bottom: 1032,
        };
        let r = indicator_rect(secondary, 1.0, 80);
        let width = r.right - r.left;
        assert_eq!(r.left, -1920 + (1920 - width) / 2);
        assert!(r.right <= 0);
    }

    #[test]
    fn coverage_is_solid_at_the_center_and_empty_outside() {
        assert_eq!(round_rect_coverage(50, 30, 100, 60, 10.0, 0.0), 1.0);
        // Well outside the top-left corner arc.
        assert_eq!(round_rect_coverage(0, 0, 100, 60, 10.0, 0.0), 0.0);
    }

    #[test]
    fn coverage_rises_monotonically_moving_inward() {
        let samples: Vec<f32> = (0..12)
            .map(|i| round_rect_coverage(i, i, 100, 60, 10.0, 0.0))
            .collect();
        for pair in samples.windows(2) {
            assert!(pair[1] >= pair[0], "coverage must rise moving inward");
        }
    }

    #[test]
    fn corner_arc_is_anti_aliased() {
        // The soft edge is a ~1 px band following the arc, so it is missed by
        // any single straight scan line (the diagonal steps over it entirely).
        // Sweep the whole corner box instead.
        let partials = (0..14)
            .flat_map(|y| (0..14).map(move |x| round_rect_coverage(x, y, 100, 60, 10.0, 0.0)))
            .filter(|c| *c > 0.0 && *c < 1.0)
            .count();
        assert!(
            partials > 0,
            "the corner arc must produce fractional coverage"
        );
    }

    #[test]
    fn inset_shrinks_the_covered_area() {
        // A pixel just inside the edge is covered at inset 0 but not at inset 2.
        let outer = round_rect_coverage(50, 1, 100, 60, 10.0, 0.0);
        let inner = round_rect_coverage(50, 1, 100, 60, 10.0, 2.0);
        assert!(outer > inner);
    }

    #[test]
    fn smoothstep_is_clamped_and_monotonic() {
        assert_eq!(smoothstep(-1.0), 0.0);
        assert_eq!(smoothstep(0.0), 0.0);
        assert_eq!(smoothstep(1.0), 1.0);
        assert_eq!(smoothstep(2.0), 1.0);
        assert!(smoothstep(0.25) < smoothstep(0.5));
        assert!(smoothstep(0.5) < smoothstep(0.75));
    }
}
