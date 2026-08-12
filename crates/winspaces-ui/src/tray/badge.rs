use std::ptr::null_mut;
use windows_sys::Win32::Graphics::Gdi::{
    CreateBitmap, CreateSolidBrush, DrawTextW, FillRect, FrameRect, GetStockObject, SetBkMode,
    SetTextColor, BLACK_BRUSH, DT_CENTER, DT_SINGLELINE, DT_VCENTER, FW_BOLD, HBITMAP, HBRUSH,
    HFONT, HGDIOBJ,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CreateIconIndirect, GetSystemMetrics, HICON, ICONINFO, SM_CXSMICON, SM_CYSMICON,
};
use winspaces_win32::gdi::color::rgb;
use winspaces_win32::gdi::font::{create_font, FACE_DISPLAY};
use winspaces_win32::gdi::guard::{DibSection, GdiObject, MemDc, ScreenDc, SelectGuard};
use winspaces_win32::text::encode_wide;

fn is_point_in_round_rect(x: i32, y: i32, w: i32, h: i32, r: i32) -> bool {
    if x < 0 || y < 0 || x >= w || y >= h {
        return false;
    }
    let cx = if x < r {
        r
    } else if x >= w - r {
        w - r - 1
    } else {
        x
    };
    let cy = if y < r {
        r
    } else if y >= h - r {
        h - r - 1
    } else {
        y
    };
    let dx = x - cx;
    let dy = y - cy;
    (dx * dx + dy * dy) <= (r * r)
}

pub(crate) fn create_fluent_badge_icon(text: &str) -> HICON {
    unsafe {
        let width = GetSystemMetrics(SM_CXSMICON).max(16);
        let height = GetSystemMetrics(SM_CYSMICON).max(16);

        // Bail cleanly on a GDI resource failure instead of the original's
        // unchecked proceed-with-null: none of these ever fail in practice
        // for an icon-sized surface, so this is inert on the success path
        // and just avoids feeding SelectObject/FillRect a null handle if
        // the impossible ever happens.
        let Some(screen_dc) = ScreenDc::new(null_mut()) else {
            return null_mut();
        };
        let Some(mem_dc) = MemDc::new(screen_dc.handle()) else {
            return null_mut();
        };
        // Top-down 32bpp BI_RGB — exactly the layout the original built by
        // hand in `bmi`.
        let Some(mut dib) = DibSection::new(screen_dc.handle(), width, height) else {
            return null_mut();
        };

        {
            // Restores hdc_mem's prior bitmap selection when this block
            // ends (right before hdc_mem itself is torn down below), same
            // as the original's `SelectObject(hdc_mem, old_bm)`.
            let _bm_guard = SelectGuard::new(mem_dc.handle(), dib.as_raw());

            // Clear background with black
            let rect_full = windows_sys::Win32::Foundation::RECT {
                left: 0,
                top: 0,
                right: width,
                bottom: height,
            };
            let brush_black = GetStockObject(BLACK_BRUSH as _);
            FillRect(mem_dc.handle(), &rect_full, brush_black as _);

            // Fill badge background: Windows 11 Dark Slate (#202020) /
            // top accent line: Windows 11 Accent Blue (#0078D4) / subtle
            // border (#3A3A3A). None of these three are ever selected into
            // the DC — FillRect/FrameRect take the brush as a direct
            // argument — so plain `GdiObject`s (no `SelectGuard`) is the
            // full fix; they drop at the end of this inner block, same
            // point the original's explicit `DeleteObject` trio ran.
            {
                let bg_brush = GdiObject::<HBRUSH>::from_raw(
                    CreateSolidBrush(rgb(0x20, 0x20, 0x20)) as HGDIOBJ,
                );
                FillRect(mem_dc.handle(), &rect_full, bg_brush.as_raw() as HBRUSH);

                let accent_brush = GdiObject::<HBRUSH>::from_raw(CreateSolidBrush(rgb(
                    0x00, 0x78, 0xD4,
                )) as HGDIOBJ);
                let rect_accent = windows_sys::Win32::Foundation::RECT {
                    left: 0,
                    top: 0,
                    right: width,
                    bottom: (height / 8).max(2),
                };
                FillRect(
                    mem_dc.handle(),
                    &rect_accent,
                    accent_brush.as_raw() as HBRUSH,
                );

                let border_brush = GdiObject::<HBRUSH>::from_raw(CreateSolidBrush(rgb(
                    0x3A, 0x3A, 0x3A,
                )) as HGDIOBJ);
                FrameRect(mem_dc.handle(), &rect_full, border_brush.as_raw() as HBRUSH);
            }

            // Typography: Bold ClearType Segoe UI Variable / Segoe UI
            let font_height = if text.len() > 3 {
                -(height * 50 / 100)
            } else if text.len() > 1 {
                -(height * 60 / 100)
            } else {
                -(height * 75 / 100)
            };

            let raw_font = {
                let f = create_font(FACE_DISPLAY, font_height, FW_BOLD as i32);
                if f.is_null() {
                    create_font("Segoe UI", font_height, FW_BOLD as i32)
                } else {
                    f
                }
            };
            let font = GdiObject::<HFONT>::from_raw(raw_font as HGDIOBJ);

            {
                let _font_guard = SelectGuard::new(mem_dc.handle(), font.as_raw());
                SetBkMode(
                    mem_dc.handle(),
                    windows_sys::Win32::Graphics::Gdi::TRANSPARENT as i32,
                );
                SetTextColor(mem_dc.handle(), rgb(0xFF, 0xFF, 0xFF));

                let mut rect_text = rect_full;
                rect_text.top += (height / 8).max(1);
                let mut wide_text = encode_wide(text);
                wide_text.pop();

                DrawTextW(
                    mem_dc.handle(),
                    wide_text.as_ptr(),
                    wide_text.len() as i32,
                    &mut rect_text,
                    DT_CENTER | DT_VCENTER | DT_SINGLELINE,
                );
            } // _font_guard restores the DC's prior font here, before `font` drops below
        } // _bm_guard restores hdc_mem's prior bitmap selection here

        // mem_dc (DeleteDC) and screen_dc (ReleaseDC) release here, before
        // the pixel fixup below — same order as the original.
        drop(mem_dc);
        drop(screen_dc);

        // 32-bit ARGB Alpha Mask Fixup (Sub-pixel Alpha Transparency)
        let corner_radius = (width / 4).max(4);
        let slice = dib.pixels();

        for y in 0..height {
            for x in 0..width {
                let idx = (y * width + x) as usize;
                let pixel = slice[idx];
                let b = (pixel & 0xFF) as u8;
                let g = ((pixel >> 8) & 0xFF) as u8;
                let r = ((pixel >> 16) & 0xFF) as u8;

                if is_point_in_round_rect(x, y, width, height, corner_radius) {
                    // Set full opacity (Alpha = 255) for pixels inside rounded squircle badge
                    slice[idx] = (255 << 24) | ((r as u32) << 16) | ((g as u32) << 8) | (b as u32);
                } else {
                    // Transparent alpha = 0 for outer corner pixels
                    slice[idx] = 0;
                }
            }
        }

        // Mask bitmap: 1-bit mask required by CreateIconIndirect
        let mask = GdiObject::<HBITMAP>::from_raw(
            CreateBitmap(width, height, 1, 1, null_mut()) as HGDIOBJ
        );

        let ii = ICONINFO {
            fIcon: 1,
            xHotspot: 0,
            yHotspot: 0,
            hbmMask: mask.as_raw() as HBITMAP,
            hbmColor: dib.as_raw() as HBITMAP,
        };

        // CreateIconIndirect copies both bitmaps internally; `dib` and
        // `mask` drop right after (reverse declaration order — `mask`
        // then `dib` — which is fine, deleting the two is order-independent
        // once the icon holds its own copies), matching the original's
        // `DeleteObject(hbm_color); DeleteObject(hbm_mask);` right after
        // this call.
        CreateIconIndirect(&ii)
    }
}
