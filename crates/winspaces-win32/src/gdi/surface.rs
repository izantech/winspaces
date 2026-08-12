//! Three surface-painting techniques, deliberately kept separate.
//!
//! `paint_surface` builds a premultiplied-alpha DIB so a background tint
//! still lets the DWM acrylic/Mica backdrop show through untouched pixels
//! (the settings-window/menu recipe). `paint_surface_clipped` is the same
//! alpha-managed contract scoped to an update rect, so a caller that knows
//! only two rows changed pays for two rows instead of the whole window.
//! `double_buffer` is an *opaque* `CreateCompatibleBitmap` double buffer
//! that does not touch alpha at all — Mission Control relies on that so
//! GDI's alpha=0 output reaches the DWM acrylic backdrop behind it
//! untouched. Do not unify these: two are alpha-managed, one deliberately
//! is not.

use std::ptr::null_mut;

use windows_sys::Win32::Foundation::RECT;
use windows_sys::Win32::Graphics::Gdi::{
    BitBlt, CreateCompatibleBitmap, CreateCompatibleDC, CreateDIBSection, DeleteDC, DeleteObject,
    SelectObject, SetBkMode, SetViewportOrgEx, BITMAPINFO, BITMAPINFOHEADER, BI_RGB,
    DIB_RGB_COLORS, HDC, SRCCOPY, TRANSPARENT,
};

use crate::gdi::color::{premultiply, Tint};

/// Paint into a premultiplied 32bpp top-down DIB filled with `tint`, run
/// `draw` against the memory DC, promote every GDI-touched (alpha=0) pixel
/// to opaque, then blit (alpha channel included) onto `target`.
///
/// Lifted from the settings window's `paint_surface`, generalized off the
/// settings-only `Palette` type onto `Tint` so this crate does not need to
/// know about settings.
///
/// # Safety
/// `target` must be a valid device context.
pub unsafe fn paint_surface<F: FnOnce(HDC)>(target: HDC, w: i32, h: i32, tint: &Tint, draw: F) {
    if w <= 0 || h <= 0 {
        return;
    }
    let hdc_mem = unsafe { CreateCompatibleDC(target) };
    let mut bmi: BITMAPINFO = unsafe { std::mem::zeroed() };
    bmi.bmiHeader.biSize = std::mem::size_of::<BITMAPINFOHEADER>() as u32;
    bmi.bmiHeader.biWidth = w;
    bmi.bmiHeader.biHeight = -h; // top-down
    bmi.bmiHeader.biPlanes = 1;
    bmi.bmiHeader.biBitCount = 32;
    bmi.bmiHeader.biCompression = BI_RGB;
    let mut bits: *mut u8 = null_mut();
    let dib = unsafe {
        CreateDIBSection(
            target,
            &bmi,
            DIB_RGB_COLORS,
            &mut bits as *mut *mut u8 as _,
            null_mut(),
            0,
        )
    };
    if dib.is_null() {
        unsafe {
            DeleteDC(hdc_mem);
        }
        return;
    }
    let old_bm = unsafe { SelectObject(hdc_mem, dib as _) };

    let alpha = tint.alpha;
    let bg_pixel = (alpha << 24)
        | (premultiply(tint.r, alpha) << 16)
        | (premultiply(tint.g, alpha) << 8)
        | premultiply(tint.b, alpha);
    let pixels = unsafe { std::slice::from_raw_parts_mut(bits as *mut u32, (w * h) as usize) };
    pixels.fill(bg_pixel);

    unsafe {
        SetBkMode(hdc_mem, TRANSPARENT as i32);
    }
    draw(hdc_mem);

    // Alpha fixup: GDI wrote alpha=0 on every pixel it touched; promote
    // those to opaque so text and highlights sit solid on the backdrop.
    for p in pixels.iter_mut() {
        if *p >> 24 == 0 {
            *p |= 0xFF00_0000;
        }
    }

    unsafe {
        // Raw copy (BitBlt preserves the alpha channel) onto the window.
        BitBlt(target, 0, 0, w, h, hdc_mem, 0, 0, SRCCOPY);

        SelectObject(hdc_mem, old_bm);
        DeleteObject(dib as _);
        DeleteDC(hdc_mem);
    }
}

/// `paint_surface` scoped to `rc`: the DIB is sized to the update rect, the
/// viewport origin is shifted (the `double_buffer` trick) so `draw` keeps
/// using absolute client coordinates and GDI clips everything outside, the
/// premultiplied tint fill and the alpha-0→opaque fixup run over the sub-rect
/// only, and the blit lands at `rc`'s position. With `rc` covering the whole
/// client area this is exactly `paint_surface`.
///
/// # Safety
/// `target` must be a valid device context.
pub unsafe fn paint_surface_clipped<F: FnOnce(HDC)>(target: HDC, rc: &RECT, tint: &Tint, draw: F) {
    let w = rc.right - rc.left;
    let h = rc.bottom - rc.top;
    if w <= 0 || h <= 0 {
        return;
    }
    let hdc_mem = unsafe { CreateCompatibleDC(target) };
    let mut bmi: BITMAPINFO = unsafe { std::mem::zeroed() };
    bmi.bmiHeader.biSize = std::mem::size_of::<BITMAPINFOHEADER>() as u32;
    bmi.bmiHeader.biWidth = w;
    bmi.bmiHeader.biHeight = -h; // top-down
    bmi.bmiHeader.biPlanes = 1;
    bmi.bmiHeader.biBitCount = 32;
    bmi.bmiHeader.biCompression = BI_RGB;
    let mut bits: *mut u8 = null_mut();
    let dib = unsafe {
        CreateDIBSection(
            target,
            &bmi,
            DIB_RGB_COLORS,
            &mut bits as *mut *mut u8 as _,
            null_mut(),
            0,
        )
    };
    if dib.is_null() {
        unsafe {
            DeleteDC(hdc_mem);
        }
        return;
    }
    let old_bm = unsafe { SelectObject(hdc_mem, dib as _) };
    unsafe {
        SetViewportOrgEx(hdc_mem, -rc.left, -rc.top, null_mut());
    }

    let alpha = tint.alpha;
    let bg_pixel = (alpha << 24)
        | (premultiply(tint.r, alpha) << 16)
        | (premultiply(tint.g, alpha) << 8)
        | premultiply(tint.b, alpha);
    let pixels = unsafe { std::slice::from_raw_parts_mut(bits as *mut u32, (w * h) as usize) };
    pixels.fill(bg_pixel);

    unsafe {
        SetBkMode(hdc_mem, TRANSPARENT as i32);
    }
    draw(hdc_mem);

    // Alpha fixup: GDI wrote alpha=0 on every pixel it touched; promote
    // those to opaque so text and highlights sit solid on the backdrop.
    for p in pixels.iter_mut() {
        if *p >> 24 == 0 {
            *p |= 0xFF00_0000;
        }
    }

    unsafe {
        // Logical coords on both sides: the shifted viewport maps
        // `(rc.left, rc.top)` to the DIB's device origin.
        BitBlt(
            target, rc.left, rc.top, w, h, hdc_mem, rc.left, rc.top, SRCCOPY,
        );

        SelectObject(hdc_mem, old_bm);
        DeleteObject(dib as _);
        DeleteDC(hdc_mem);
    }
}

/// Opaque double buffer: draw into a `CreateCompatibleBitmap` sized to
/// `rc`, with the viewport origin shifted so `draw` can keep using absolute
/// client coordinates while GDI clips everything outside `rc`, then blit
/// onto `target`. Alpha is NOT touched — unlike `paint_surface`, this
/// technique is for surfaces that must let GDI's alpha=0 output reach a DWM
/// backdrop untouched. Falls back to drawing directly on `target` if the
/// compatible DC/bitmap can't be created.
///
/// Verbatim from Mission Control's `WM_PAINT`.
///
/// # Safety
/// `target` must be a valid device context.
pub unsafe fn double_buffer<F: FnOnce(HDC)>(target: HDC, rc: &RECT, draw: F) {
    let width = rc.right - rc.left;
    let height = rc.bottom - rc.top;
    if width <= 0 || height <= 0 {
        return;
    }
    let mem_dc = unsafe { CreateCompatibleDC(target) };
    let mem_bmp = unsafe { CreateCompatibleBitmap(target, width, height) };
    if !mem_dc.is_null() && !mem_bmp.is_null() {
        unsafe {
            let old_bmp = SelectObject(mem_dc, mem_bmp as _);
            SetViewportOrgEx(mem_dc, -rc.left, -rc.top, null_mut());
            draw(mem_dc);
            BitBlt(
                target, rc.left, rc.top, width, height, mem_dc, rc.left, rc.top, SRCCOPY,
            );
            SelectObject(mem_dc, old_bmp);
        }
    } else {
        draw(target);
    }
    unsafe {
        if !mem_bmp.is_null() {
            DeleteObject(mem_bmp as _);
        }
        if !mem_dc.is_null() {
            DeleteDC(mem_dc);
        }
    }
}
