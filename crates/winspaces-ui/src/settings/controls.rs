//! Fluent chrome for the settings window: owner-drawn toggle, button, field
//! and pill controls, plus the focus ring and font set they share. These are
//! app-specific widgets, not primitives — the GDI primitives they compose
//! (`paint_surface`, `fill_round`, `draw_text_in`, `draw_glyph_in`,
//! `measure_text`, `create_font`) live in `winspaces_win32` and are called
//! from here and from `settings_ui/mod.rs` directly.

use crate::theme::Palette;
use windows_sys::Win32::Foundation::RECT;
use windows_sys::Win32::Graphics::Gdi::{
    CreatePen, CreateSolidBrush, DeleteObject, Ellipse, GetStockObject, RoundRect, DT_CENTER,
    DT_END_ELLIPSIS, DT_LEFT, DT_SINGLELINE, DT_VCENTER, HBRUSH, HDC, HFONT, HGDIOBJ, HPEN,
    NULL_BRUSH, PS_SOLID,
};

use winspaces_win32::gdi::draw::{draw_glyph_in, draw_text_in, fill_round};
use winspaces_win32::gdi::font::{create_font, FACE_DISPLAY, FACE_ICONS, FACE_TEXT};
use winspaces_win32::gdi::guard::{GdiObject, SelectGuard};
use winspaces_win32::glyphs::GLYPH_CHEVRON_DOWN;

pub struct Fonts {
    pub title: HFONT,
    pub subtitle: HFONT,
    pub body: HFONT,
    pub body_strong: HFONT,
    pub caption: HFONT,
    pub glyph: HFONT,
    pub glyph_small: HFONT,
}

pub unsafe fn create_fonts(scale: f32) -> Fonts {
    let px = |v: i32| (v as f32 * scale).round() as i32;
    Fonts {
        title: create_font(FACE_DISPLAY, -px(28), 600),
        subtitle: create_font(FACE_DISPLAY, -px(20), 600),
        body: create_font(FACE_TEXT, -px(14), 400),
        body_strong: create_font(FACE_TEXT, -px(14), 600),
        caption: create_font(FACE_TEXT, -px(12), 400),
        glyph: create_font(FACE_ICONS, -px(18), 400),
        glyph_small: create_font(FACE_ICONS, -px(12), 400),
    }
}

pub unsafe fn delete_fonts(fonts: &Fonts) {
    for f in [
        fonts.title,
        fonts.subtitle,
        fonts.body,
        fonts.body_strong,
        fonts.caption,
        fonts.glyph,
        fonts.glyph_small,
    ] {
        DeleteObject(f as _);
    }
}

#[derive(Clone, Copy, Default)]
pub struct Vis {
    pub hover: bool,
    pub pressed: bool,
    pub focus: bool,
}

/// 2px keyboard focus ring just outside the control.
pub unsafe fn draw_focus_ring(hdc: HDC, r: &RECT, pal: &Palette, px: i32) {
    let pen =
        GdiObject::<HPEN>::from_raw(CreatePen(PS_SOLID, (2 * px).max(2), pal.focus) as HGDIOBJ);
    let pad = 2 * px;
    {
        let _pen_guard = SelectGuard::new(hdc, pen.as_raw());
        let hollow = GetStockObject(NULL_BRUSH);
        let _brush_guard = SelectGuard::new(hdc, hollow);
        RoundRect(
            hdc,
            r.left - pad,
            r.top - pad,
            r.right + pad,
            r.bottom + pad,
            6 * px,
            6 * px,
        );
    } // guards restore the DC's prior pen/brush here, before `pen` drops below
}

/// Win11-style toggle switch. `r` is the 40x20 (dip) track rect.
pub unsafe fn draw_toggle(hdc: HDC, r: &RECT, on: bool, vis: Vis, pal: &Palette, px: i32) {
    let h = r.bottom - r.top;
    let radius = h;
    if on {
        fill_round(hdc, r, radius, pal.accent, pal.accent);
    } else {
        let fill = if vis.hover || vis.pressed {
            pal.ctl_hover
        } else {
            pal.ctl
        };
        fill_round(hdc, r, radius, fill, pal.text_dim);
    }

    // Knob: 12dip circle, 14dip when hovered (Win11 grows the knob on hover).
    let knob_d = if vis.hover || vis.pressed {
        14 * px
    } else {
        12 * px
    };
    let margin = (h - knob_d) / 2;
    let (kl, kr) = if on {
        (r.right - margin - knob_d, r.right - margin)
    } else {
        (r.left + margin, r.left + margin + knob_d)
    };
    let knob_color = if on { pal.accent_text } else { pal.text_dim };
    let brush = GdiObject::<HBRUSH>::from_raw(CreateSolidBrush(knob_color) as HGDIOBJ);
    let pen = GdiObject::<HPEN>::from_raw(CreatePen(PS_SOLID, 1, knob_color) as HGDIOBJ);
    {
        let _brush_guard = SelectGuard::new(hdc, brush.as_raw());
        let _pen_guard = SelectGuard::new(hdc, pen.as_raw());
        Ellipse(hdc, kl, r.top + margin, kr, r.bottom - margin);
    } // guards restore the DC's prior brush/pen here, before brush/pen drop below

    if vis.focus {
        draw_focus_ring(hdc, r, pal, px);
    }
}

/// Push button; `accent` selects the filled accent variant.
#[allow(clippy::too_many_arguments)]
pub unsafe fn draw_button(
    hdc: HDC,
    r: &RECT,
    label: &str,
    accent: bool,
    vis: Vis,
    pal: &Palette,
    fonts: &Fonts,
    px: i32,
) {
    let (fill, stroke, text) = if accent {
        (pal.accent, pal.accent, pal.accent_text)
    } else if vis.pressed {
        (pal.ctl_pressed, pal.ctl_stroke, pal.text_dim)
    } else if vis.hover {
        (pal.ctl_hover, pal.ctl_stroke, pal.text)
    } else {
        (pal.ctl, pal.ctl_stroke, pal.text)
    };
    fill_round(hdc, r, 4 * px, fill, stroke);
    draw_text_in(
        hdc,
        fonts.body,
        text,
        r,
        label,
        DT_SINGLELINE | DT_VCENTER | DT_CENTER | DT_END_ELLIPSIS,
    );
    if vis.focus {
        draw_focus_ring(hdc, r, pal, px);
    }
}

/// Field used by both the hotkey recorder and the theme combo: a button-like
/// well with a value label. `capturing` swaps in the accent capture affordance;
/// `chevron` appends the combo dropdown glyph.
#[allow(clippy::too_many_arguments)]
pub unsafe fn draw_field(
    hdc: HDC,
    r: &RECT,
    label: &str,
    capturing: bool,
    chevron: bool,
    vis: Vis,
    pal: &Palette,
    fonts: &Fonts,
    px: i32,
) {
    let (fill, stroke) = if capturing {
        (pal.ctl, pal.accent)
    } else if vis.pressed {
        (pal.ctl_pressed, pal.ctl_stroke)
    } else if vis.hover {
        (pal.ctl_hover, pal.ctl_stroke)
    } else {
        (pal.ctl, pal.ctl_stroke)
    };
    fill_round(hdc, r, 4 * px, fill, stroke);
    let text_color = if capturing { pal.text_dim } else { pal.text };
    let mut text_rect = RECT {
        left: r.left + 12 * px,
        top: r.top,
        right: r.right - if chevron { 28 * px } else { 12 * px },
        bottom: r.bottom,
    };
    draw_text_in(
        hdc,
        fonts.body,
        text_color,
        &text_rect,
        label,
        DT_SINGLELINE | DT_VCENTER | DT_LEFT | DT_END_ELLIPSIS,
    );
    if chevron {
        text_rect.left = r.right - 26 * px;
        text_rect.right = r.right - 8 * px;
        draw_glyph_in(
            hdc,
            fonts.glyph_small,
            pal.text_dim,
            &text_rect,
            GLYPH_CHEVRON_DOWN,
            DT_SINGLELINE | DT_VCENTER | DT_CENTER,
        );
    }
    if vis.focus {
        draw_focus_ring(hdc, r, pal, px);
    }
}

/// Status pill: rounded badge with a colored dot and caption text.
#[allow(clippy::too_many_arguments)]
pub unsafe fn draw_pill(
    hdc: HDC,
    r: &RECT,
    text: &str,
    dot: u32,
    bg: u32,
    pal: &Palette,
    fonts: &Fonts,
    px: i32,
) {
    let h = r.bottom - r.top;
    fill_round(hdc, r, h, bg, bg);
    let dot_d = 8 * px;
    let dot_x = r.left + 12 * px;
    let dot_y = r.top + (h - dot_d) / 2;
    let brush = GdiObject::<HBRUSH>::from_raw(CreateSolidBrush(dot) as HGDIOBJ);
    let pen = GdiObject::<HPEN>::from_raw(CreatePen(PS_SOLID, 1, dot) as HGDIOBJ);
    {
        let _brush_guard = SelectGuard::new(hdc, brush.as_raw());
        let _pen_guard = SelectGuard::new(hdc, pen.as_raw());
        Ellipse(hdc, dot_x, dot_y, dot_x + dot_d, dot_y + dot_d);
    } // guards restore the DC's prior brush/pen here, before brush/pen drop below
    let text_rect = RECT {
        left: dot_x + dot_d + 8 * px,
        top: r.top,
        right: r.right - 12 * px,
        bottom: r.bottom,
    };
    draw_text_in(
        hdc,
        fonts.caption,
        pal.text,
        &text_rect,
        text,
        DT_SINGLELINE | DT_VCENTER | DT_LEFT | DT_END_ELLIPSIS,
    );
}
