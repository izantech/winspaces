//! Pure geometry: spaces-bar layout, drag-slot resolution, hit-testing
//! rectangles. No Win32 except the `RECT`/`POINT` types themselves — this is
//! the concrete testability win of the crate split, and takes all of Mission
//! Control's unit tests.

use windows_sys::Win32::Foundation::{POINT, RECT};
use winspaces_win32::dpi;

// Deliberately local and INCLUSIVE (`<=` on right/bottom), unlike the menu's
// and settings window's exclusive `<` hit test. The kit ships no shared
// `pt_in_rect` for exactly this reason — unifying the two semantics would
// shift Mission Control's close-button and card-edge hit targets by a pixel.
pub(crate) fn pt_in_rect(rect: &RECT, pt: POINT) -> bool {
    pt.x >= rect.left && pt.x <= rect.right && pt.y >= rect.top && pt.y <= rect.bottom
}

/// Resolved geometry for the spaces bar strip.
pub(crate) struct SpacesBarMetrics {
    pub(crate) card_w: i32,
    pub(crate) card_h: i32,
    pub(crate) gap: i32,
    pub(crate) start_x: i32,
    pub(crate) top_y: i32,
    /// Zeroed when `has_plus` was false.
    pub(crate) plus_rect: RECT,
}

/// Centered-strip layout for `count` space cards plus an optional "+" tile.
/// Pure so the overflow clamp is unit-testable: when the natural width would
/// not fit the monitor (9 cards on a narrow display at high DPI), card width
/// shrinks toward a floor instead of `start_x` going negative and pushing
/// cards off both edges.
pub(crate) fn spaces_bar_metrics(
    count: usize,
    has_plus: bool,
    width: i32,
    scale: f32,
    plus_label_w: i32,
) -> SpacesBarMetrics {
    let px = |val: i32| dpi::px(scale, val);
    let count = count.max(1) as i32;
    let card_h = px(100);
    let gap = px(16);
    let top_y = px(28);
    // Wide enough for the glyph over its "New Space" label, still visibly
    // narrower than a space card so the row reads as "N spaces, then an
    // action" rather than N+1 spaces. `plus_label_w` is the measured width
    // of the translated label in device pixels (0 when unknown): a longer
    // language widens the tile instead of ellipsizing it.
    let plus_w = px(132).max(plus_label_w + px(24));
    let margin = px(60);

    let plus_total = if has_plus { plus_w + gap } else { 0 };
    let avail = width - 2 * margin;
    let mut card_w = px(210);
    if count * card_w + (count - 1) * gap + plus_total > avail {
        card_w = ((avail - plus_total - (count - 1) * gap) / count).max(px(120));
    }

    let total_w = count * card_w + (count - 1) * gap + plus_total;
    let start_x = (width - total_w) / 2;
    let plus_left = start_x + count * (card_w + gap);
    let plus_rect = if has_plus {
        RECT {
            left: plus_left,
            top: top_y,
            right: plus_left + plus_w,
            bottom: top_y + card_h,
        }
    } else {
        RECT {
            left: 0,
            top: 0,
            right: 0,
            bottom: 0,
        }
    };

    SpacesBarMetrics {
        card_w,
        card_h,
        gap,
        start_x,
        top_y,
        plus_rect,
    }
}

/// Given the horizontal center of a dragged space card, the start_x of the
/// spaces bar, the card width, gap, and total number of cards, computes which
/// slot (0..count-1) the card would land in. Transitions at the exact midpoints
/// between adjacent card slots. Pure so it is unit-testable.
pub(crate) fn target_slot_for_center(
    center_x: i32,
    start_x: i32,
    card_w: i32,
    gap: i32,
    count: usize,
) -> usize {
    if count <= 1 {
        return 0;
    }
    let pitch = card_w + gap;
    if pitch <= 0 {
        return 0;
    }
    let rel_x = center_x - start_x;
    let slot = (rel_x + gap / 2) / pitch;
    slot.clamp(0, (count - 1) as i32) as usize
}

/// The band of the overlay the spaces bar occupies, padded for the floating
/// card a drag lifts `px(4)` above the row. Nothing else moves while a space
/// card is dragged, so this is the only region those frames need to repaint.
/// Pure so the padding is unit-testable.
pub(crate) fn spaces_bar_strip_rect(bar: &SpacesBarMetrics, width: i32, scale: f32) -> RECT {
    let pad = (8.0 * scale).round() as i32;
    RECT {
        left: 0,
        top: bar.top_y - pad,
        right: width,
        bottom: bar.top_y + bar.card_h + pad,
    }
}

/// Left edge of the floating card a space drag lifts: the card's resting
/// position slid by `delta_x`, kept inside the overlay's margins. Shared by
/// the renderer and the drag handler so the region invalidated for a frame is
/// exactly the region that frame redraws. Pure so the clamp is unit-testable.
pub(crate) fn floating_card_left(
    card_left: i32,
    delta_x: i32,
    card_w: i32,
    width: i32,
    scale: f32,
) -> i32 {
    let margin = dpi::px(scale, 20);
    let max_left = width - card_w - margin;
    // Not `clamp`: on a client area too narrow for one card plus margins
    // `max_left` drops below `margin`, and `clamp` panics when its bounds
    // cross — inside `WM_PAINT`, in a daemon.
    (card_left + delta_x).min(max_left).max(margin)
}

/// The slot `delta` steps from `current`, or `None` at the ends. Backs the
/// `Ctrl+Shift+←/→` keyboard equivalent of dragging a space card — resolved by
/// the host's `reorder_space_neighbor`, which is the only side that can read
/// the live `current`.
pub fn neighbor_slot(current: usize, delta: i32, count: usize) -> Option<usize> {
    let target = current as i32 + delta;
    if target < 0 || target >= count as i32 {
        return None;
    }
    Some(target as usize)
}

/// Circular close button in a space card's top-right corner. Shared by render
/// and hit-testing so the drawn button and the clickable area cannot disagree.
pub(crate) fn close_button_rect(card: &RECT, scale: f32) -> RECT {
    let px = |val: i32| dpi::px(scale, val);
    let d = px(20);
    let pad = px(6);
    RECT {
        left: card.right - pad - d,
        top: card.top + pad,
        right: card.right - pad,
        bottom: card.top + pad + d,
    }
}

/// Circular close button in a window card's top-right corner.
pub(crate) fn window_close_button_rect(card: &RECT, scale: f32) -> RECT {
    let px = |val: i32| dpi::px(scale, val);
    let d = px(20);
    let pad = px(8);
    RECT {
        left: card.right - pad - d,
        top: card.top + pad,
        right: card.right - pad,
        bottom: card.top + pad + d,
    }
}

/// Circular pin/sticky button in a window card's header, immediately to the left of the close button.
pub(crate) fn window_pin_button_rect(card: &RECT, scale: f32) -> RECT {
    let px = |val: i32| dpi::px(scale, val);
    let d = px(20);
    let pad = px(8);
    let gap = px(6);
    let right = card.right - pad - d - gap;
    RECT {
        left: right - d,
        top: card.top + pad,
        right,
        bottom: card.top + pad + d,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn window_pin_button_rect_sits_to_left_of_close_button() {
        let card = RECT {
            left: 100,
            top: 100,
            right: 400,
            bottom: 300,
        };
        let close = window_close_button_rect(&card, 1.0);
        let pin = window_pin_button_rect(&card, 1.0);
        assert_eq!(pin.right, close.left - 6);
        assert_eq!(pin.top, close.top);
        assert_eq!(pin.bottom, close.bottom);
        assert_eq!(pin.right - pin.left, 20);
    }

    fn rect_w(r: &RECT) -> i32 {
        r.right - r.left
    }

    #[test]
    fn spaces_bar_fits_four_cards_at_natural_width() {
        let m = spaces_bar_metrics(4, true, 1920, 1.0, 0);
        assert_eq!(m.card_w, 210);
        assert!(m.start_x > 0);
        // Plus tile sits one gap after the last card.
        assert_eq!(m.plus_rect.left, m.start_x + 4 * (m.card_w + m.gap));
        assert_eq!(rect_w(&m.plus_rect), 132);
    }

    #[test]
    fn plus_tile_grows_with_a_long_label() {
        let short = spaces_bar_metrics(4, true, 1920, 1.0, 60);
        assert_eq!(rect_w(&short.plus_rect), 132);
        let long = spaces_bar_metrics(4, true, 1920, 1.0, 160);
        assert_eq!(rect_w(&long.plus_rect), 184);
    }

    #[test]
    fn spaces_bar_shrinks_cards_instead_of_overflowing() {
        // Nine cards at 210px + gaps exceed 1920px; the clamp must keep the
        // strip inside the margins rather than letting start_x go negative.
        let m = spaces_bar_metrics(9, false, 1920, 1.0, 0);
        assert!(m.card_w < 210);
        assert!(m.card_w >= 120);
        assert!(m.start_x >= 0);
        let total = 9 * m.card_w + 8 * m.gap;
        assert!(total <= 1920 - 2 * 60);
    }

    #[test]
    fn spaces_bar_clamp_accounts_for_the_plus_tile() {
        // Same width: adding the plus tile must shrink cards further, never
        // push the tile past the margin.
        let without = spaces_bar_metrics(8, false, 1600, 1.0, 0);
        let with = spaces_bar_metrics(8, true, 1600, 1.0, 0);
        assert!(with.card_w <= without.card_w);
        assert!(with.plus_rect.right <= 1600 - 60);
    }

    #[test]
    fn spaces_bar_at_max_count_has_no_plus_rect() {
        let m = spaces_bar_metrics(9, false, 3840, 1.5, 0);
        assert_eq!(rect_w(&m.plus_rect), 0);
    }

    #[test]
    fn close_button_sits_inside_the_card_corner() {
        let card = RECT {
            left: 100,
            top: 28,
            right: 310,
            bottom: 128,
        };
        let cb = close_button_rect(&card, 1.0);
        assert!(cb.left > card.left && cb.right <= card.right);
        assert!(cb.top >= card.top && cb.bottom < card.bottom);
        // Scale grows the button with the card.
        let cb2 = close_button_rect(&card, 2.0);
        assert_eq!(cb2.bottom - cb2.top, 2 * (cb.bottom - cb.top));
    }

    #[test]
    fn target_slot_for_center_resolves_correct_slots_and_transitions_at_midpoints() {
        // 4 cards with start_x = 100, card_w = 200, gap = 20 (pitch = 220).
        // Slot centers:
        // Slot 0: 200 (range < 310)
        // Slot 1: 420 (range 310..530)
        // Slot 2: 640 (range 530..750)
        // Slot 3: 860 (range >= 750)
        let start_x = 100;
        let card_w = 200;
        let gap = 20;
        let count = 4;

        // Centered on slot 0:
        assert_eq!(target_slot_for_center(200, start_x, card_w, gap, count), 0);
        // Offscreen / clamped left:
        assert_eq!(target_slot_for_center(-100, start_x, card_w, gap, count), 0);
        // Just before midpoint to slot 1 (309):
        assert_eq!(target_slot_for_center(309, start_x, card_w, gap, count), 0);
        // Exact midpoint to slot 1 (310):
        assert_eq!(target_slot_for_center(310, start_x, card_w, gap, count), 1);
        // Centered on slot 1 (420):
        assert_eq!(target_slot_for_center(420, start_x, card_w, gap, count), 1);
        // Just before midpoint to slot 2 (529):
        assert_eq!(target_slot_for_center(529, start_x, card_w, gap, count), 1);
        // Midpoint to slot 2 (530):
        assert_eq!(target_slot_for_center(530, start_x, card_w, gap, count), 2);
        // Centered on slot 3 (860):
        assert_eq!(target_slot_for_center(860, start_x, card_w, gap, count), 3);
        // Far right / clamped to max slot:
        assert_eq!(target_slot_for_center(2000, start_x, card_w, gap, count), 3);
    }

    #[test]
    fn bar_strip_spans_the_width_and_covers_the_lifted_card() {
        let bar = spaces_bar_metrics(4, true, 1920, 1.0, 0);
        let strip = spaces_bar_strip_rect(&bar, 1920, 1.0);
        assert_eq!(strip.left, 0);
        assert_eq!(strip.right, 1920);
        // The floating card is drawn px(4) above the row; the strip must
        // include it, or a drag would smear its top edge.
        assert!(strip.top <= bar.top_y - 4);
        assert!(strip.bottom >= bar.top_y + bar.card_h);

        // Padding scales with DPI, like everything else in the bar.
        let bar2 = spaces_bar_metrics(4, true, 3840, 2.0, 0);
        let strip2 = spaces_bar_strip_rect(&bar2, 3840, 2.0);
        assert_eq!(bar2.top_y - strip2.top, 2 * (bar.top_y - strip.top));
    }

    #[test]
    fn floating_card_stays_inside_the_overlay_margins() {
        // 1920-wide overlay, 210-wide card, 20px margins at scale 1.
        assert_eq!(floating_card_left(100, 50, 210, 1920, 1.0), 150);
        assert_eq!(floating_card_left(100, -500, 210, 1920, 1.0), 20);
        assert_eq!(floating_card_left(1600, 500, 210, 1920, 1.0), 1690);
        // Margins scale with DPI, like the rest of the bar.
        assert_eq!(floating_card_left(0, -500, 210, 1920, 2.0), 40);
        // A client area too narrow for one card crosses the clamp bounds; it
        // must pin to the left margin, not panic inside `WM_PAINT`.
        assert_eq!(floating_card_left(0, 999, 210, 100, 1.0), 20);
    }

    #[test]
    fn neighbor_slot_stops_at_both_ends() {
        assert_eq!(neighbor_slot(0, 1, 4), Some(1));
        assert_eq!(neighbor_slot(3, -1, 4), Some(2));
        assert_eq!(neighbor_slot(0, -1, 4), None);
        assert_eq!(neighbor_slot(3, 1, 4), None);
        // Single-space monitor: nowhere to go in either direction.
        assert_eq!(neighbor_slot(0, -1, 1), None);
        assert_eq!(neighbor_slot(0, 1, 1), None);
    }

    #[test]
    fn window_close_button_rect_sits_in_top_right() {
        let card = RECT {
            left: 100,
            top: 200,
            right: 400,
            bottom: 450,
        };
        let close = window_close_button_rect(&card, 1.0);
        assert_eq!(close.right, 400 - 8);
        assert_eq!(close.left, 400 - 8 - 20);
        assert_eq!(close.top, 200 + 8);
        assert_eq!(close.bottom, 200 + 8 + 20);
    }
}
