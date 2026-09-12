//! Painting a `MenuWindow` into a premultiplied DIB via `winspaces_win32::gdi`.

use super::{MenuEntry, MenuState, MenuWindow};
use super::{BG_ALPHA, CHEVRON_W, HOVER_INSET, HOVER_RADIUS, ICON_X, RIGHT_PAD, TEXT_X};
use windows_sys::Win32::Foundation::RECT;
use windows_sys::Win32::Graphics::Gdi::{
    FillRect, SelectObject, SetTextColor, DT_CENTER, DT_END_ELLIPSIS, DT_NOPREFIX, DT_RIGHT,
    DT_SINGLELINE, DT_VCENTER, HDC,
};
use winspaces_win32::dpi::px;
use winspaces_win32::gdi::color::Tint;
use winspaces_win32::gdi::draw::{draw_text_raw, round_rect_with};
use winspaces_win32::gdi::guard::SelectGuard;
use winspaces_win32::gdi::surface::paint_surface_clipped;
use winspaces_win32::glyphs::{GLYPH_CHECK, GLYPH_CHEVRON};

// `draw_text_raw` deliberately does not add `DT_NOPREFIX` itself (Overview's
// contract disagrees) — the menu keeps OR-ing it in here, as it always has.
unsafe fn draw_text(hdc: HDC, text: &str, rect: &mut RECT, flags: u32) {
    draw_text_raw(hdc, text, rect, flags | DT_NOPREFIX);
}

unsafe fn draw_glyph(hdc: HDC, glyph: u16, rect: &mut RECT, flags: u32) {
    let mut ch = String::with_capacity(1);
    ch.push(char::from_u32(glyph as u32).unwrap_or('\u{FFFD}'));
    draw_text_raw(hdc, &ch, rect, flags | DT_NOPREFIX);
}

/// Render one menu window into a 32bpp premultiplied DIB and blit it, alpha
/// channel included, onto the window surface. Background pixels carry
/// `BG_ALPHA` so the DWM acrylic shows through; everything GDI touched
/// (text, highlights, separators — GDI zeroes alpha) is fixed up to opaque.
/// The DIB build/fill/alpha-fixup/blit steps are `gdi::paint_surface_clipped`,
/// scoped to `rc` (the `WM_PAINT` update rect — hover changes invalidate only
/// the affected rows, so most paints touch two rows, not the window); only
/// the foreground GDI drawing below is menu-specific, and GDI clips it.
pub(crate) unsafe fn render_window(win: &MenuWindow, state: &MenuState, target: HDC, rc: &RECT) {
    let w = win.width;
    let px = |v: i32| px(state.scale, v);

    let theme = &state.theme;
    let tint = Tint {
        r: theme.bg_r,
        g: theme.bg_g,
        b: theme.bg_b,
        // BG_ALPHA stays menu-specific — not the settings window's base_alpha.
        alpha: if state.acrylic { BG_ALPHA } else { 255 },
    };

    paint_surface_clipped(target, rc, &tint, |hdc_mem| {
        // Created once per open in `show_custom`, destroyed in `close_menu`.
        let hover_brush = state.paints.hover_brush;
        let hover_pen = state.paints.hover_pen;
        let sep_brush = state.paints.sep_brush;

        let any_chevron = win
            .entries
            .iter()
            .any(|e| matches!(e, MenuEntry::Item(it) if it.submenu.is_some()));

        for (idx, entry) in win.entries.iter().enumerate() {
            let row = win.rows[idx];
            match entry {
                MenuEntry::Header(text) => {
                    SelectObject(hdc_mem, state.fonts.small as _);
                    SetTextColor(hdc_mem, theme.dim);
                    let mut r = RECT {
                        left: px(ICON_X),
                        top: row.top,
                        right: w - px(RIGHT_PAD),
                        bottom: row.top + row.height,
                    };
                    draw_text(
                        hdc_mem,
                        text,
                        &mut r,
                        DT_SINGLELINE | DT_VCENTER | DT_END_ELLIPSIS,
                    );
                }
                MenuEntry::Separator => {
                    let line_y = row.top + row.height / 2;
                    let r = RECT {
                        left: px(12),
                        top: line_y,
                        right: w - px(12),
                        bottom: line_y + px(1).max(1),
                    };
                    FillRect(hdc_mem, &r, sep_brush);
                }
                MenuEntry::Item(it) => {
                    let highlighted = win.hover == Some(idx) || win.sel == Some(idx);
                    if highlighted {
                        let hover_r = RECT {
                            left: px(HOVER_INSET),
                            top: row.top + px(2),
                            right: w - px(HOVER_INSET),
                            bottom: row.top + row.height - px(2),
                        };
                        // Tightly scoped: restores the DC's prior brush/pen
                        // right after the shape draws, so the shared paint
                        // objects are never left selected in this DC when the
                        // paint ends (`close_menu` deletes them later).
                        let _brush_guard = SelectGuard::new(hdc_mem, hover_brush as _);
                        let _pen_guard = SelectGuard::new(hdc_mem, hover_pen as _);
                        round_rect_with(
                            hdc_mem,
                            &hover_r,
                            hover_brush,
                            hover_pen,
                            px(HOVER_RADIUS),
                        );
                    }

                    // Icon column: checkmark wins over the item glyph.
                    let icon_glyph = if it.checked {
                        Some(GLYPH_CHECK)
                    } else {
                        it.glyph
                    };
                    if let Some(g) = icon_glyph {
                        SelectObject(hdc_mem, state.fonts.glyph as _);
                        SetTextColor(hdc_mem, theme.text);
                        let mut r = RECT {
                            left: px(ICON_X),
                            top: row.top,
                            right: px(TEXT_X) - px(6),
                            bottom: row.top + row.height,
                        };
                        draw_glyph(hdc_mem, g, &mut r, DT_SINGLELINE | DT_VCENTER | DT_CENTER);
                    }

                    let chevron_space = if any_chevron { px(CHEVRON_W) } else { 0 };
                    let right_edge = w - px(RIGHT_PAD) - chevron_space;

                    // Label.
                    SelectObject(hdc_mem, state.fonts.text as _);
                    SetTextColor(hdc_mem, theme.text);
                    let mut r = RECT {
                        left: px(TEXT_X),
                        top: row.top,
                        right: right_edge,
                        bottom: row.top + row.height,
                    };
                    draw_text(
                        hdc_mem,
                        &it.label,
                        &mut r,
                        DT_SINGLELINE | DT_VCENTER | DT_END_ELLIPSIS,
                    );

                    // Shortcut / secondary text, right-aligned and dimmed.
                    if let Some(shortcut) = &it.shortcut {
                        SelectObject(hdc_mem, state.fonts.small as _);
                        SetTextColor(hdc_mem, theme.dim);
                        let mut r = RECT {
                            left: px(TEXT_X),
                            top: row.top,
                            right: right_edge,
                            bottom: row.top + row.height,
                        };
                        draw_text(
                            hdc_mem,
                            shortcut,
                            &mut r,
                            DT_SINGLELINE | DT_VCENTER | DT_RIGHT,
                        );
                    }

                    if it.submenu.is_some() {
                        SelectObject(hdc_mem, state.fonts.glyph_small as _);
                        SetTextColor(hdc_mem, theme.dim);
                        let mut r = RECT {
                            left: w - px(RIGHT_PAD) - px(16),
                            top: row.top,
                            right: w - px(RIGHT_PAD) + px(4),
                            bottom: row.top + row.height,
                        };
                        draw_glyph(
                            hdc_mem,
                            GLYPH_CHEVRON,
                            &mut r,
                            DT_SINGLELINE | DT_VCENTER | DT_CENTER,
                        );
                    }
                }
            }
        }
    });
}
