//! Shared drawing primitives.
//!
//! Two `DrawTextW` contracts live here on purpose: `draw_text_in` /
//! `draw_glyph_in` select the font and text color before drawing (the
//! settings-window contract, and bake in `DT_NOPREFIX`); `draw_text_raw`
//! does neither — no font/color select, no `DT_NOPREFIX` added — because
//! menu.rs and Mission Control disagree on `DT_NOPREFIX` (menu adds it,
//! Mission Control doesn't) and both already select font/color themselves
//! as part of a larger per-window paint batch. `draw_text_raw` passes
//! `flags` through unchanged so each caller decides for itself. Do not
//! merge the two contracts.
//!
//! Deliberately NOT shipped here: `pt_in_rect`. Two incompatible semantics
//! exist in the codebase today — Mission Control's is inclusive (`<=` on
//! right and bottom), the menu's and the settings window's are exclusive.
//! Unifying them would silently shift Mission Control's hit targets by a
//! pixel. Giving them no shared home removes the temptation; each surface
//! keeps its own.

use windows_sys::Win32::Foundation::{RECT, SIZE};
use windows_sys::Win32::Graphics::Gdi::{
    CreatePen, CreateSolidBrush, DeleteObject, DrawTextW, GetTextExtentPoint32W, RoundRect,
    SelectObject, SetTextColor, DT_NOPREFIX, HBRUSH, HDC, HFONT, HPEN, PS_SOLID,
};

/// Fill+stroke a rounded rectangle with a freshly created brush/pen,
/// restoring the DC's previous selection before deleting them.
///
/// # Safety
/// `hdc` must be a valid device context.
pub unsafe fn fill_round(hdc: HDC, r: &RECT, radius: i32, fill: u32, stroke: u32) {
    unsafe {
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
}

/// Fill+stroke a rounded rectangle with an already-selected brush/pen — the
/// caller owns their lifetime (created once and reused across many cards,
/// as Mission Control's space-card chrome does). Unlike `fill_round`, this
/// never creates or deletes a GDI object.
///
/// # Safety
/// `hdc` must be a valid device context; `brush` and `pen` must be valid,
/// live GDI objects.
pub unsafe fn round_rect_with(hdc: HDC, r: &RECT, brush: HBRUSH, pen: HPEN, radius: i32) {
    unsafe {
        SelectObject(hdc, brush as _);
        SelectObject(hdc, pen as _);
        RoundRect(hdc, r.left, r.top, r.right, r.bottom, radius, radius);
    }
}

/// Draw text, selecting `font` and `color` first. Bakes in `DT_NOPREFIX`
/// (the settings-window contract).
///
/// # Safety
/// `hdc` must be a valid device context; `font` must be a valid, live font
/// handle.
pub unsafe fn draw_text_in(hdc: HDC, font: HFONT, color: u32, r: &RECT, text: &str, flags: u32) {
    let wide: Vec<u16> = text.encode_utf16().collect();
    let mut rect = *r;
    unsafe {
        SelectObject(hdc, font as _);
        SetTextColor(hdc, color);
        DrawTextW(
            hdc,
            wide.as_ptr(),
            wide.len() as i32,
            &mut rect,
            flags | DT_NOPREFIX,
        );
    }
}

/// Draw a single glyph, selecting `font` and `color` first. Bakes in
/// `DT_NOPREFIX`.
///
/// # Safety
/// `hdc` must be a valid device context; `font` must be a valid, live font
/// handle.
pub unsafe fn draw_glyph_in(hdc: HDC, font: HFONT, color: u32, r: &RECT, glyph: u16, flags: u32) {
    let wide = [glyph];
    let mut rect = *r;
    unsafe {
        SelectObject(hdc, font as _);
        SetTextColor(hdc, color);
        DrawTextW(hdc, wide.as_ptr(), 1, &mut rect, flags | DT_NOPREFIX);
    }
}

/// Draw text with `flags` passed straight through: no font/color select, no
/// `DT_NOPREFIX` added. See the module doc for why this is the contract
/// menu.rs and Mission Control share instead of `draw_text_in`.
///
/// # Safety
/// `hdc` must be a valid device context with a font already selected.
pub unsafe fn draw_text_raw(hdc: HDC, text: &str, rect: &mut RECT, flags: u32) {
    let wide: Vec<u16> = text.encode_utf16().collect();
    unsafe {
        DrawTextW(hdc, wide.as_ptr(), wide.len() as i32, rect, flags);
    }
}

/// Height in device pixels of `text` set in `font` and word-wrapped to
/// `width`, as `DrawTextW(DT_WORDBREAK)` would lay it out. Uses
/// `DT_EDITCONTROL` so a partially visible last line is never counted.
///
/// # Safety
/// `hdc` must be a valid device context; `font` must be a valid, live font
/// handle.
pub unsafe fn measure_text_wrapped(hdc: HDC, font: HFONT, text: &str, width: i32) -> i32 {
    use windows_sys::Win32::Graphics::Gdi::{DT_CALCRECT, DT_EDITCONTROL, DT_WORDBREAK};
    let wide: Vec<u16> = text.encode_utf16().collect();
    let mut rect = RECT {
        left: 0,
        top: 0,
        right: width.max(1),
        bottom: 0,
    };
    unsafe {
        SelectObject(hdc, font as _);
        DrawTextW(
            hdc,
            wide.as_ptr(),
            wide.len() as i32,
            &mut rect,
            DT_CALCRECT | DT_WORDBREAK | DT_EDITCONTROL | DT_NOPREFIX,
        );
    }
    rect.bottom - rect.top
}

/// Measure `text` set in `font`, returning its width in device pixels.
///
/// # Safety
/// `hdc` must be a valid device context; `font` must be a valid, live font
/// handle.
pub unsafe fn measure_text(hdc: HDC, font: HFONT, text: &str) -> i32 {
    let wide: Vec<u16> = text.encode_utf16().collect();
    let mut size = SIZE { cx: 0, cy: 0 };
    unsafe {
        SelectObject(hdc, font as _);
        GetTextExtentPoint32W(hdc, wide.as_ptr(), wide.len() as i32, &mut size);
    }
    size.cx
}
