//! Owner-drawn Fluent control kit for the settings window: pure GDI drawing
//! helpers with no state of their own. Every function paints into the DIB
//! surface created by `paint_surface` (the menu.rs recipe: premultiplied
//! 32bpp top-down DIB, manual alpha management, BitBlt with alpha preserved).

use super::theme::Palette;
use std::ptr::null_mut;
use windows_sys::Win32::Foundation::{RECT, SIZE};
use windows_sys::Win32::Graphics::Gdi::{
    BitBlt, CreateCompatibleDC, CreateDIBSection, CreateFontW, CreatePen, CreateSolidBrush,
    DeleteDC, DeleteObject, DrawTextW, Ellipse, GetTextExtentPoint32W, RoundRect, SelectObject,
    SetBkMode, SetTextColor, BITMAPINFO, BITMAPINFOHEADER, BI_RGB, CLEARTYPE_QUALITY,
    CLIP_DEFAULT_PRECIS, DEFAULT_CHARSET, DEFAULT_PITCH, DIB_RGB_COLORS, DT_CENTER,
    DT_END_ELLIPSIS, DT_LEFT, DT_NOPREFIX, DT_SINGLELINE, DT_VCENTER, HDC, HFONT,
    OUT_DEFAULT_PRECIS, PS_SOLID, SRCCOPY, TRANSPARENT,
};

use crate::tray::encode_wide;

pub struct Fonts {
    pub title: HFONT,
    pub subtitle: HFONT,
    pub body: HFONT,
    pub body_strong: HFONT,
    pub caption: HFONT,
    pub glyph: HFONT,
    pub glyph_small: HFONT,
}

unsafe fn create_font(face: &str, height: i32, weight: i32) -> HFONT {
    let name = encode_wide(face);
    CreateFontW(
        -height,
        0,
        0,
        0,
        weight,
        0,
        0,
        0,
        DEFAULT_CHARSET as u32,
        OUT_DEFAULT_PRECIS as u32,
        CLIP_DEFAULT_PRECIS as u32,
        CLEARTYPE_QUALITY as u32,
        DEFAULT_PITCH as u32,
        name.as_ptr(),
    )
}

pub unsafe fn create_fonts(scale: f32) -> Fonts {
    let px = |v: i32| (v as f32 * scale).round() as i32;
    Fonts {
        title: create_font("Segoe UI Variable Display", px(28), 600),
        subtitle: create_font("Segoe UI Variable Display", px(20), 600),
        body: create_font("Segoe UI Variable Text", px(14), 400),
        body_strong: create_font("Segoe UI Variable Text", px(14), 600),
        caption: create_font("Segoe UI Variable Text", px(12), 400),
        glyph: create_font("Segoe Fluent Icons", px(18), 400),
        glyph_small: create_font("Segoe Fluent Icons", px(12), 400),
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

/// Create the premultiplied DIB, fill the theme base tint, run `draw` against
/// the memory DC, promote every GDI-touched (alpha=0) pixel to opaque, and
/// blit onto `target`. The base tint keeps alpha < 255 over Mica so the
/// backdrop shows through untouched areas.
pub unsafe fn paint_surface<F: FnOnce(HDC)>(target: HDC, w: i32, h: i32, pal: &Palette, draw: F) {
    if w <= 0 || h <= 0 {
        return;
    }
    let hdc_mem = CreateCompatibleDC(target);
    let mut bmi: BITMAPINFO = std::mem::zeroed();
    bmi.bmiHeader.biSize = std::mem::size_of::<BITMAPINFOHEADER>() as u32;
    bmi.bmiHeader.biWidth = w;
    bmi.bmiHeader.biHeight = -h;
    bmi.bmiHeader.biPlanes = 1;
    bmi.bmiHeader.biBitCount = 32;
    bmi.bmiHeader.biCompression = BI_RGB;
    let mut bits: *mut u8 = null_mut();
    let dib = CreateDIBSection(
        target,
        &bmi,
        DIB_RGB_COLORS,
        &mut bits as *mut *mut u8 as _,
        null_mut(),
        0,
    );
    if dib.is_null() {
        DeleteDC(hdc_mem);
        return;
    }
    let old_bm = SelectObject(hdc_mem, dib as _);

    let alpha = pal.base_alpha;
    let pm = |c: u32| c * alpha / 255;
    let bg_pixel = (alpha << 24) | (pm(pal.base_r) << 16) | (pm(pal.base_g) << 8) | pm(pal.base_b);
    let pixels = std::slice::from_raw_parts_mut(bits as *mut u32, (w * h) as usize);
    pixels.fill(bg_pixel);

    SetBkMode(hdc_mem, TRANSPARENT as i32);
    draw(hdc_mem);

    for p in pixels.iter_mut() {
        if *p >> 24 == 0 {
            *p |= 0xFF00_0000;
        }
    }

    BitBlt(target, 0, 0, w, h, hdc_mem, 0, 0, SRCCOPY);
    SelectObject(hdc_mem, old_bm);
    DeleteObject(dib as _);
    DeleteDC(hdc_mem);
}

pub unsafe fn fill_round(hdc: HDC, r: &RECT, radius: i32, fill: u32, stroke: u32) {
    let brush = CreateSolidBrush(fill);
    let pen = CreatePen(PS_SOLID, 1, stroke);
    let old_b = SelectObject(hdc, brush as _);
    let old_p = SelectObject(hdc, pen as _);
    RoundRect(hdc, r.left, r.top, r.right, r.bottom, radius, radius);
    SelectObject(hdc, old_b);
    SelectObject(hdc, old_p);
    DeleteObject(brush as _);
    DeleteObject(pen as _);
}

pub unsafe fn draw_text_in(hdc: HDC, font: HFONT, color: u32, r: &RECT, text: &str, flags: u32) {
    SelectObject(hdc, font as _);
    SetTextColor(hdc, color);
    let wide: Vec<u16> = text.encode_utf16().collect();
    let mut rect = *r;
    DrawTextW(
        hdc,
        wide.as_ptr(),
        wide.len() as i32,
        &mut rect,
        flags | DT_NOPREFIX,
    );
}

pub unsafe fn draw_glyph_in(hdc: HDC, font: HFONT, color: u32, r: &RECT, glyph: u16, flags: u32) {
    SelectObject(hdc, font as _);
    SetTextColor(hdc, color);
    let wide = [glyph];
    let mut rect = *r;
    DrawTextW(hdc, wide.as_ptr(), 1, &mut rect, flags | DT_NOPREFIX);
}

pub unsafe fn measure_text(hdc: HDC, font: HFONT, text: &str) -> i32 {
    SelectObject(hdc, font as _);
    let wide: Vec<u16> = text.encode_utf16().collect();
    let mut size = SIZE { cx: 0, cy: 0 };
    GetTextExtentPoint32W(hdc, wide.as_ptr(), wide.len() as i32, &mut size);
    size.cx
}

#[derive(Clone, Copy, Default)]
pub struct Vis {
    pub hover: bool,
    pub pressed: bool,
    pub focus: bool,
}

/// 2px keyboard focus ring just outside the control.
pub unsafe fn draw_focus_ring(hdc: HDC, r: &RECT, pal: &Palette, px: i32) {
    let pen = CreatePen(PS_SOLID, (2 * px).max(2), pal.focus);
    let old_p = SelectObject(hdc, pen as _);
    let hollow = windows_sys::Win32::Graphics::Gdi::GetStockObject(
        windows_sys::Win32::Graphics::Gdi::NULL_BRUSH,
    );
    let old_b = SelectObject(hdc, hollow);
    let pad = 2 * px;
    RoundRect(
        hdc,
        r.left - pad,
        r.top - pad,
        r.right + pad,
        r.bottom + pad,
        6 * px,
        6 * px,
    );
    SelectObject(hdc, old_p);
    SelectObject(hdc, old_b);
    DeleteObject(pen as _);
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
    let brush = CreateSolidBrush(knob_color);
    let pen = CreatePen(PS_SOLID, 1, knob_color);
    let old_b = SelectObject(hdc, brush as _);
    let old_p = SelectObject(hdc, pen as _);
    Ellipse(hdc, kl, r.top + margin, kr, r.bottom - margin);
    SelectObject(hdc, old_b);
    SelectObject(hdc, old_p);
    DeleteObject(brush as _);
    DeleteObject(pen as _);

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
            0xE70D, // ChevronDown
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
    let brush = CreateSolidBrush(dot);
    let pen = CreatePen(PS_SOLID, 1, dot);
    let old_b = SelectObject(hdc, brush as _);
    let old_p = SelectObject(hdc, pen as _);
    Ellipse(hdc, dot_x, dot_y, dot_x + dot_d, dot_y + dot_d);
    SelectObject(hdc, old_b);
    SelectObject(hdc, old_p);
    DeleteObject(brush as _);
    DeleteObject(pen as _);
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
