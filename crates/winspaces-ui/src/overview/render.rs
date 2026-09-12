//! `WM_PAINT`: the spaces bar (resting, dragging, and drop-target states) and
//! the overview window-card grid.

use super::geometry::{close_button_rect, floating_card_left, spaces_bar_metrics};
use super::{Overview, SpaceCard, OVERVIEW_STATE};
use std::ptr::null_mut;
use windows_sys::Win32::Foundation::{HWND, RECT};
use windows_sys::Win32::Graphics::Gdi::{
    CreatePen, CreateSolidBrush, Ellipse, FillRect, RoundRect, SelectObject, SetBkMode,
    SetTextColor, DT_CENTER, DT_END_ELLIPSIS, DT_SINGLELINE, DT_VCENTER, HBRUSH, HDC, HGDIOBJ,
    HPEN, PS_DASH, PS_SOLID, TRANSPARENT,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{DrawIconEx, GetClientRect, DI_NORMAL};
use winspaces_common::i18n::{t, tn};
use winspaces_common::{tr, Msg, PluralMsg};
use winspaces_win32::dpi;
use winspaces_win32::gdi::color::rgb;
use winspaces_win32::gdi::draw::{draw_text_raw, round_rect_with};
use winspaces_win32::gdi::guard::{GdiObject, SelectGuard};
use winspaces_win32::glyphs::{GLYPH_ADD, GLYPH_PIN, GLYPH_PIN_FILLED};

pub(crate) unsafe fn render_overview(hdc: HDC, hwnd: HWND) {
    OVERVIEW_STATE.with(|s| {
        let mc = s.borrow();
        let mut client_rect: RECT = std::mem::zeroed();
        GetClientRect(hwnd, &mut client_rect);

        let scale = mc.scale;
        let px = |val: i32| dpi::px(scale, val);

        // 1. Dark Acrylic Background Fill
        {
            let bg_brush =
                GdiObject::<HBRUSH>::from_raw(CreateSolidBrush(rgb(0x14, 0x14, 0x18)) as HGDIOBJ);
            FillRect(hdc, &client_rect, bg_brush.as_raw() as HBRUSH);
        } // deleted here, same as the original's immediate DeleteObject

        SetBkMode(hdc, TRANSPARENT as i32);

        // 2. Render Spaces Bar (Top)
        //
        // Scoped to this block: every brush/pen below is only used through
        // this point, so its `GdiObject` drop lands exactly where the old
        // manual `DeleteObject` batch used to (right before the window-card
        // grid starts drawing), instead of drifting to the end of the
        // function.
        {
            let card_bg =
                GdiObject::<HBRUSH>::from_raw(CreateSolidBrush(rgb(0x1F, 0x1F, 0x24)) as HGDIOBJ);
            let card_active_bg =
                GdiObject::<HBRUSH>::from_raw(CreateSolidBrush(rgb(0x28, 0x2A, 0x40)) as HGDIOBJ); // Accent Indigo tint
            let card_hover_bg =
                GdiObject::<HBRUSH>::from_raw(CreateSolidBrush(rgb(0x2C, 0x2C, 0x36)) as HGDIOBJ);

            let placeholder_bg =
                GdiObject::<HBRUSH>::from_raw(CreateSolidBrush(rgb(0x16, 0x16, 0x1C)) as HGDIOBJ); // Vacated drop slot
            let plus_bg =
                GdiObject::<HBRUSH>::from_raw(CreateSolidBrush(rgb(0x17, 0x17, 0x1D)) as HGDIOBJ); // "New Space" tile
            let plus_hover_bg =
                GdiObject::<HBRUSH>::from_raw(CreateSolidBrush(rgb(0x1E, 0x20, 0x30)) as HGDIOBJ); // Indigo-tinted

            let pen_border =
                GdiObject::<HPEN>::from_raw(
                    CreatePen(PS_SOLID, 1, rgb(0x38, 0x38, 0x42)) as HGDIOBJ
                );
            let pen_active =
                GdiObject::<HPEN>::from_raw(
                    CreatePen(PS_SOLID, px(2), rgb(0x81, 0x8C, 0xF8)) as HGDIOBJ
                ); // Indigo Accent Outline
            let pen_drag_target =
                GdiObject::<HPEN>::from_raw(
                    CreatePen(PS_SOLID, px(2), rgb(0x34, 0xD3, 0x99)) as HGDIOBJ
                ); // Emerald Green
            let pen_placeholder =
                GdiObject::<HPEN>::from_raw(
                    CreatePen(PS_SOLID, 1, rgb(0x48, 0x48, 0x58)) as HGDIOBJ
                );
            // Dashed: the tile is an empty slot waiting to be filled, not a card.
            let pen_plus = GdiObject::<HPEN>::from_raw(
                CreatePen(PS_DASH, 1, rgb(0x4A, 0x4A, 0x58)) as HGDIOBJ,
            );
            let close_bg =
                GdiObject::<HBRUSH>::from_raw(CreateSolidBrush(rgb(0x3A, 0x3A, 0x44)) as HGDIOBJ);
            let close_bg_hover =
                GdiObject::<HBRUSH>::from_raw(CreateSolidBrush(rgb(0xE8, 0x55, 0x5A)) as HGDIOBJ); // Destructive Red

            let r_corner = px(12);

            // Deferred to the very end of the bar so nothing overlaps it.
            let mut floating_card: Option<(usize, RECT)> = None;

            if mc.drag_space_active {
                let from_idx = mc.dragging_space.unwrap_or(0);
                let target_slot = mc.drag_space_target_slot.unwrap_or(from_idx);
                let count = mc.space_cards.len();
                let width = client_rect.right - client_rect.left;
                let bar = spaces_bar_metrics(count, mc.plus_visible, width, scale, mc.plus_label_w);

                let slot_rect = |slot: usize| {
                    let left = bar.start_x + slot as i32 * (bar.card_w + bar.gap);
                    RECT {
                        left,
                        top: bar.top_y,
                        right: left + bar.card_w,
                        bottom: bar.top_y + bar.card_h,
                    }
                };

                // 2a. Placeholder Slot at target_slot
                draw_space_card(
                    hdc,
                    &slot_rect(target_slot),
                    placeholder_bg.as_raw() as HBRUSH,
                    pen_placeholder.as_raw() as HPEN,
                    r_corner,
                );

                // 2b. Non-dragged cards shifted to their new slots
                for (idx, card) in mc.space_cards.iter().enumerate() {
                    if idx == from_idx {
                        continue;
                    }
                    let slot_idx = winspaces_core::spaces::remap_index_after_reorder(
                        idx,
                        from_idx,
                        target_slot,
                    );
                    let card_rect = slot_rect(slot_idx);

                    let (brush, pen): (&GdiObject<HBRUSH>, &GdiObject<HPEN>) = if card.is_active {
                        (&card_active_bg, &pen_active)
                    } else {
                        (&card_bg, &pen_border)
                    };
                    draw_space_card(
                        hdc,
                        &card_rect,
                        brush.as_raw() as HBRUSH,
                        pen.as_raw() as HPEN,
                        r_corner,
                    );
                    draw_space_card_text(hdc, &mc, card, &card_rect, scale);
                }

                // 2c. The dragged card itself is drawn last of all — after the
                // "new space" tile below — so it passes *over* every other tile in
                // the bar instead of sliding underneath one. It tracks the cursor
                // horizontally but stays pinned to the bar's y: DWM composites the
                // thumbnail grid *above* this window's GDI output, so a card
                // dragged down over the grid would vanish behind it.
                if mc.space_cards.get(from_idx).is_some() {
                    let delta_x = mc.drag_space_current_x - mc.drag_offset.x;
                    let drag_left = floating_card_left(
                        mc.space_cards[from_idx].rect.left,
                        delta_x,
                        bar.card_w,
                        width,
                        scale,
                    );
                    let drag_top = bar.top_y - px(4);
                    floating_card = Some((
                        from_idx,
                        RECT {
                            left: drag_left,
                            top: drag_top,
                            right: drag_left + bar.card_w,
                            bottom: drag_top + bar.card_h,
                        },
                    ));
                }
            } else {
                for (idx, card) in mc.space_cards.iter().enumerate() {
                    let is_hover = mc.hovered_space == Some(idx);
                    let is_drag_target = mc.drag_active && is_hover;

                    let brush: &GdiObject<HBRUSH> = if is_drag_target {
                        &card_hover_bg
                    } else if card.is_active {
                        &card_active_bg
                    } else if is_hover {
                        &card_hover_bg
                    } else {
                        &card_bg
                    };

                    let pen: &GdiObject<HPEN> = if is_drag_target {
                        &pen_drag_target
                    } else if card.is_active {
                        &pen_active
                    } else {
                        &pen_border
                    };

                    draw_space_card(
                        hdc,
                        &card.rect,
                        brush.as_raw() as HBRUSH,
                        pen.as_raw() as HPEN,
                        r_corner,
                    );
                    draw_space_card_text(hdc, &mc, card, &card.rect, scale);

                    // Close button, macOS style: only on the hovered card, and never
                    // when this is the monitor's last space.
                    if is_hover && mc.space_cards.len() > 1 {
                        let cb = close_button_rect(&card.rect, scale);
                        let is_close_hover = mc.hovered_close == Some(idx);
                        let close_brush: &GdiObject<HBRUSH> = if is_close_hover {
                            &close_bg_hover
                        } else {
                            &close_bg
                        };
                        {
                            // Tightly scoped: restores the DC's prior brush/pen
                            // the moment the shape is drawn, so close_bg /
                            // close_bg_hover / pen_border are never still
                            // selected when their `GdiObject`s drop below.
                            let _brush_guard = SelectGuard::new(hdc, close_brush.as_raw());
                            let _pen_guard = SelectGuard::new(hdc, pen_border.as_raw());
                            let d = cb.right - cb.left;
                            // Corner radius = diameter renders the round rect as a circle.
                            RoundRect(hdc, cb.left, cb.top, cb.right, cb.bottom, d, d);
                        }
                        SelectObject(hdc, mc.h_font_close);
                        SetTextColor(hdc, rgb(0xFF, 0xFF, 0xFF));
                        let mut x_rect = cb;
                        draw_text_raw(
                            hdc,
                            "✕",
                            &mut x_rect,
                            DT_CENTER | DT_SINGLELINE | DT_VCENTER,
                        );
                    }
                }
            }

            // The "new space" tile: deliberately *not* in the space cards' visual
            // family. A dimmer, dashed ghost slot reads as an action at the end of
            // the row rather than an N+1'th space — the old flat "+" in card
            // colours was too easy to mistake for one. Emerald drag outline when a
            // window drag hovers it (drop = new space + move).
            if mc.plus_visible {
                let is_hover = mc.hovered_plus;
                let is_drag_target = mc.drag_active && is_hover;
                let brush: &GdiObject<HBRUSH> = if is_hover { &plus_hover_bg } else { &plus_bg };
                let pen: &GdiObject<HPEN> = if is_drag_target {
                    &pen_drag_target
                } else if is_hover {
                    &pen_active
                } else {
                    &pen_plus
                };
                draw_space_card(
                    hdc,
                    &mc.plus_rect,
                    brush.as_raw() as HBRUSH,
                    pen.as_raw() as HPEN,
                    r_corner,
                );

                SetTextColor(
                    hdc,
                    if is_drag_target {
                        rgb(0x34, 0xD3, 0x99)
                    } else if is_hover {
                        rgb(0xC7, 0xD2, 0xFE)
                    } else {
                        rgb(0x8A, 0x8A, 0x96)
                    },
                );

                // Same Fluent "Add" mark the tray menu's New Space item uses.
                SelectObject(hdc, mc.h_font_glyph);
                let glyph = char::from_u32(GLYPH_ADD as u32)
                    .map(String::from)
                    .unwrap_or_else(|| "+".to_string());
                let mut glyph_rect = RECT {
                    left: mc.plus_rect.left,
                    top: mc.plus_rect.top + px(20),
                    right: mc.plus_rect.right,
                    bottom: mc.plus_rect.top + px(56),
                };
                draw_text_raw(
                    hdc,
                    &glyph,
                    &mut glyph_rect,
                    DT_CENTER | DT_SINGLELINE | DT_VCENTER,
                );

                SelectObject(hdc, mc.h_font_small);
                let mut label_rect = RECT {
                    left: mc.plus_rect.left + px(4),
                    top: mc.plus_rect.top + px(56),
                    right: mc.plus_rect.right - px(4),
                    bottom: mc.plus_rect.bottom - px(12),
                };
                draw_text_raw(
                    hdc,
                    t(Msg::OverviewNewSpace),
                    &mut label_rect,
                    DT_CENTER | DT_SINGLELINE | DT_VCENTER | DT_END_ELLIPSIS,
                );
            }

            // Last in the bar, so a dragged card rides over the tiles it passes.
            if let Some((idx, drag_rect)) = floating_card {
                if let Some(card) = mc.space_cards.get(idx) {
                    draw_space_card(
                        hdc,
                        &drag_rect,
                        card_active_bg.as_raw() as HBRUSH,
                        pen_active.as_raw() as HPEN,
                        r_corner,
                    );
                    draw_space_card_text(hdc, &mc, card, &drag_rect, scale);
                }
            }
        } // `card_bg`, `card_active_bg`, ..., `close_bg_hover` (13 objects)
          // drop here — same point the old manual `DeleteObject` batch ran,
          // now automatic via `GdiObject`'s `Drop`.

        // 3. Render Window Cards (Overview Grid)
        if mc.window_cards.is_empty() {
            SelectObject(hdc, mc.h_font_title);
            SetTextColor(hdc, rgb(0x71, 0x71, 0x7A));
            let empty_msg = tr!(Msg::OverviewEmpty, n = mc.active_space_idx + 1);
            let mut center_rect = RECT {
                left: client_rect.left,
                top: client_rect.top + px(220),
                right: client_rect.right,
                bottom: client_rect.bottom,
            };
            draw_text_raw(
                hdc,
                &empty_msg,
                &mut center_rect,
                DT_CENTER | DT_SINGLELINE | DT_VCENTER | DT_END_ELLIPSIS,
            );
            return;
        }

        let win_card_bg =
            GdiObject::<HBRUSH>::from_raw(CreateSolidBrush(rgb(0x1F, 0x1F, 0x24)) as HGDIOBJ);
        let win_card_hover_bg =
            GdiObject::<HBRUSH>::from_raw(CreateSolidBrush(rgb(0x28, 0x28, 0x32)) as HGDIOBJ);
        let win_pen =
            GdiObject::<HPEN>::from_raw(CreatePen(PS_SOLID, 1, rgb(0x38, 0x38, 0x42)) as HGDIOBJ);
        let win_pen_hover =
            GdiObject::<HPEN>::from_raw(
                CreatePen(PS_SOLID, px(2), rgb(0x81, 0x8C, 0xF8)) as HGDIOBJ
            ); // Indigo Highlight

        let card_corner = px(14);
        let icon_size = px(20);

        for (idx, card) in mc.window_cards.iter().enumerate() {
            let is_dragged = mc.drag_active && mc.dragging_window == Some(idx);
            let is_hover = !is_dragged && mc.hovered_window == Some(idx);
            let brush: &GdiObject<HBRUSH> = if is_hover {
                &win_card_hover_bg
            } else {
                &win_card_bg
            };
            let pen: &GdiObject<HPEN> = if is_hover { &win_pen_hover } else { &win_pen };

            {
                // Restores the DC's prior brush/pen right after the shape
                // is drawn, so `win_card_bg`/`win_pen` (etc.) are never
                // still selected when their `GdiObject`s drop below.
                let _brush_guard = SelectGuard::new(hdc, brush.as_raw());
                let _pen_guard = SelectGuard::new(hdc, pen.as_raw());
                RoundRect(
                    hdc,
                    card.card_rect.left,
                    card.card_rect.top,
                    card.card_rect.right,
                    card.card_rect.bottom,
                    card_corner,
                    card_corner,
                );
            }

            // Draw Real Window Icon if available
            let header_h = px(38);
            let icon_x = card.card_rect.left + px(12);
            let icon_y = card.card_rect.top + (header_h - icon_size) / 2;
            if !card.h_icon.is_null() {
                DrawIconEx(
                    hdc,
                    icon_x,
                    icon_y,
                    card.h_icon,
                    icon_size,
                    icon_size,
                    0,
                    null_mut(),
                    DI_NORMAL,
                );
            }

            // Window Header Title Text (dimmed on the lifted source card)
            SelectObject(hdc, mc.h_font_card);
            let title_color = if is_dragged {
                rgb(0x71, 0x71, 0x7A)
            } else {
                rgb(0xFF, 0xFF, 0xFF)
            };
            SetTextColor(hdc, title_color);
            let text_left = if !card.h_icon.is_null() {
                icon_x + icon_size + px(8)
            } else {
                card.card_rect.left + px(14)
            };

            let title_right_pad = if is_hover {
                px(60)
            } else if card.is_sticky {
                px(36)
            } else {
                px(14)
            };
            let mut title_r = RECT {
                left: text_left,
                top: card.card_rect.top,
                right: card.card_rect.right - title_right_pad,
                bottom: card.card_rect.top + header_h,
            };
            draw_text_raw(
                hdc,
                &card.title,
                &mut title_r,
                DT_SINGLELINE | DT_VCENTER | DT_END_ELLIPSIS,
            );

            // Window Card Action Buttons on Hover
            if is_hover {
                let close_r = super::geometry::window_close_button_rect(&card.card_rect, scale);
                let is_close_hover = mc.hovered_window_close == Some(idx);
                let (c_bg, c_text) = if is_close_hover {
                    (rgb(0xE8, 0x55, 0x5A), rgb(0xFF, 0xFF, 0xFF)) // Destructive Red
                } else {
                    (rgb(0x3A, 0x3A, 0x44), rgb(0xD4, 0xD4, 0xD8))
                };
                let c_brush = GdiObject::<HBRUSH>::from_raw(CreateSolidBrush(c_bg) as HGDIOBJ);
                let c_pen = GdiObject::<HPEN>::from_raw(CreatePen(PS_SOLID, 1, c_bg) as HGDIOBJ);
                {
                    let _b_guard = SelectGuard::new(hdc, c_brush.as_raw());
                    let _p_guard = SelectGuard::new(hdc, c_pen.as_raw());
                    Ellipse(
                        hdc,
                        close_r.left,
                        close_r.top,
                        close_r.right,
                        close_r.bottom,
                    );
                }
                SelectObject(hdc, mc.h_font_close);
                SetTextColor(hdc, c_text);
                let mut text_r = close_r;
                draw_text_raw(
                    hdc,
                    "✕",
                    &mut text_r,
                    DT_SINGLELINE | DT_CENTER | DT_VCENTER,
                );

                // Pin / Sticky Button
                let pin_r = super::geometry::window_pin_button_rect(&card.card_rect, scale);
                let is_pin_hover = mc.hovered_window_pin == Some(idx);
                let (pin_bg, pin_color) = if card.is_sticky {
                    if is_pin_hover {
                        (rgb(0x4F, 0x46, 0xE5), rgb(0xFF, 0xFF, 0xFF))
                    } else {
                        (rgb(0x37, 0x30, 0xA3), rgb(0xC7, 0xD2, 0xFE))
                    }
                } else if is_pin_hover {
                    (rgb(0x4A, 0x4A, 0x58), rgb(0xFF, 0xFF, 0xFF))
                } else {
                    (rgb(0x3A, 0x3A, 0x44), rgb(0xD4, 0xD4, 0xD8))
                };
                let pin_brush = GdiObject::<HBRUSH>::from_raw(CreateSolidBrush(pin_bg) as HGDIOBJ);
                let pin_pen =
                    GdiObject::<HPEN>::from_raw(CreatePen(PS_SOLID, 1, pin_bg) as HGDIOBJ);
                {
                    let _b_guard = SelectGuard::new(hdc, pin_brush.as_raw());
                    let _p_guard = SelectGuard::new(hdc, pin_pen.as_raw());
                    Ellipse(hdc, pin_r.left, pin_r.top, pin_r.right, pin_r.bottom);
                }
                SelectObject(hdc, mc.h_font_pin);
                SetTextColor(hdc, pin_color);
                // Filled while pinned, outline while not: the mark alone says
                // which state the card is in, so the affordance still reads on
                // a thumbnail whose colours happen to sit near the button's.
                let pin_glyph = if card.is_sticky {
                    GLYPH_PIN_FILLED
                } else {
                    GLYPH_PIN
                };
                let glyph = char::from_u32(pin_glyph as u32)
                    .map(String::from)
                    .unwrap_or_else(|| "P".to_string());
                let mut pin_text_r = pin_r;
                draw_text_raw(
                    hdc,
                    &glyph,
                    &mut pin_text_r,
                    DT_SINGLELINE | DT_CENTER | DT_VCENTER,
                );
            } else if card.is_sticky {
                // Resting badge: bare glyph, no circle behind it. A pinned card
                // has to stay legible as pinned when the pointer is elsewhere,
                // but drawing the full button would advertise a target that is
                // not hit-tested until the card is hovered.
                let badge_r = super::geometry::window_close_button_rect(&card.card_rect, scale);
                SelectObject(hdc, mc.h_font_pin);
                SetTextColor(hdc, rgb(0x81, 0x8C, 0xF8)); // Indigo Accent
                let glyph = char::from_u32(GLYPH_PIN_FILLED as u32)
                    .map(String::from)
                    .unwrap_or_else(|| "P".to_string());
                let mut badge_text_r = badge_r;
                draw_text_raw(
                    hdc,
                    &glyph,
                    &mut badge_text_r,
                    DT_SINGLELINE | DT_CENTER | DT_VCENTER,
                );
            }
        }

        // win_card_bg, win_card_hover_bg, win_pen, win_pen_hover drop here
        // (closure end), same point the old manual DeleteObject batch ran.
    });
}

/// A space card's chrome: the rounded body in `brush`, outlined in `pen`.
/// Shared by the resting cards, the shifted cards a drag previews, the drop
/// placeholder and the floating card, so they can never drift apart.
///
/// `SelectGuard` pre-selects `brush`/`pen` and restores the DC's prior
/// selection when this call returns. `round_rect_with` (win32) then
/// harmlessly re-selects the same, already-selected handles before drawing
/// — same draw, same order, just guaranteed not to leave `brush`/`pen`
/// still selected once the caller's `GdiObject` deletes them.
unsafe fn draw_space_card(hdc: HDC, rect: &RECT, brush: HBRUSH, pen: HPEN, r_corner: i32) {
    let _brush_guard = SelectGuard::new(hdc, brush as HGDIOBJ);
    let _pen_guard = SelectGuard::new(hdc, pen as HGDIOBJ);
    round_rect_with(hdc, rect, brush, pen, r_corner);
}

unsafe fn draw_space_card_text(
    hdc: HDC,
    mc: &Overview,
    card: &SpaceCard,
    card_rect: &RECT,
    scale: f32,
) {
    let px = |val: i32| dpi::px(scale, val);

    // Title: "Space X"
    SelectObject(hdc, mc.h_font_title);
    SetTextColor(hdc, rgb(0xFF, 0xFF, 0xFF));
    let title_text = tr!(Msg::OverviewSpace, n = card.space_idx + 1);
    let mut title_rect = RECT {
        left: card_rect.left + px(16),
        top: card_rect.top + px(18),
        right: card_rect.right - px(16),
        bottom: card_rect.top + px(48),
    };
    draw_text_raw(
        hdc,
        &title_text,
        &mut title_rect,
        DT_CENTER | DT_SINGLELINE | DT_VCENTER,
    );

    // Subtitle: "N windows" or "Active"
    SelectObject(hdc, mc.h_font_small);
    let sub_color = if card.is_active {
        rgb(0xC7, 0xD2, 0xFE)
    } else {
        rgb(0x9C, 0x9C, 0xA4)
    };
    SetTextColor(hdc, sub_color);

    let win_str = tn(
        PluralMsg::OverviewWindowCount,
        card.window_count as u64,
        &[("n", &card.window_count)],
    );
    let tiled_tag = if card.is_tiled {
        t(Msg::OverviewTiledTag)
    } else {
        ""
    };
    let rest = format!("{win_str}{tiled_tag}");

    let sub_text = if card.is_active {
        tr!(Msg::OverviewSubtitleActive, rest = rest)
    } else {
        rest
    };

    let mut sub_rect = RECT {
        left: card_rect.left + px(12),
        top: card_rect.top + px(52),
        right: card_rect.right - px(12),
        bottom: card_rect.bottom - px(14),
    };
    // Ellipsize: on a monitor narrow enough to push cards to their width
    // floor, "Active • 12 windows" no longer fits the card.
    draw_text_raw(
        hdc,
        &sub_text,
        &mut sub_rect,
        DT_CENTER | DT_SINGLELINE | DT_VCENTER | DT_END_ELLIPSIS,
    );
}
