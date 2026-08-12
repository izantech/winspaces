//! Font creation shared by every owner-drawn surface.

use windows_sys::Win32::Graphics::Gdi::{
    CreateFontW, CLEARTYPE_QUALITY, CLIP_DEFAULT_PRECIS, DEFAULT_CHARSET, DEFAULT_PITCH, HFONT,
    OUT_DEFAULT_PRECIS,
};

use crate::text::encode_wide;

/// Segoe UI Variable Display — headings and titles.
pub const FACE_DISPLAY: &str = "Segoe UI Variable Display";
/// Segoe UI Variable Text — body copy.
pub const FACE_TEXT: &str = "Segoe UI Variable Text";
/// Segoe Fluent Icons — glyph-font iconography (codepoints in `crate::glyphs`).
pub const FACE_ICONS: &str = "Segoe Fluent Icons";

/// Create a GDI font.
///
/// `height` is passed to `CreateFontW` exactly as given — **the caller owns
/// the sign**. Mission Control passes a POSITIVE cell height; the menu,
/// settings window and tray badge pass a NEGATED character height.
/// Normalizing the sign here would change Mission Control's glyph sizes on
/// every surface it draws, so this function never does.
///
/// Always requests `CLEARTYPE_QUALITY` / `DEFAULT_CHARSET` /
/// `OUT_DEFAULT_PRECIS` / `CLIP_DEFAULT_PRECIS` / `DEFAULT_PITCH`. This is a
/// deliberate, user-approved change for Mission Control, which previously
/// passed 0 (`DEFAULT_QUALITY`, no antialiasing) for all four — glyph sizes
/// are unaffected, only the rasterizer gains ClearType.
///
/// # Safety
/// Thin FFI wrapper around `CreateFontW`; no handle safety requirements
/// beyond a valid `face` string, which is guaranteed by taking `&str`.
pub unsafe fn create_font(face: &str, height: i32, weight: i32) -> HFONT {
    let name = encode_wide(face);
    unsafe {
        CreateFontW(
            height,
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
}
