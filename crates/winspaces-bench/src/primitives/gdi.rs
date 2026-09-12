//! `gdi/*`: the menu's memory-DC allocate + alpha fixup + blit, and the
//! opaque double buffer Overview repaints through. Every surface
//! drawn into here is a memory DC compatible with the screen, never the
//! screen DC itself, so nothing reaches the display.

use windows_sys::Win32::Foundation::RECT;
use windows_sys::Win32::Graphics::Gdi::{
    CreateCompatibleBitmap, CreateCompatibleDC, DeleteDC, DeleteObject, GetDC, ReleaseDC,
    SelectObject, HBITMAP, HDC, HGDIOBJ,
};
use winspaces_win32::gdi::color::Tint;
use winspaces_win32::gdi::surface::{double_buffer, paint_surface};

use crate::timing::Runner;

/// A memory DC compatible with the screen, with a same-size bitmap selected
/// in. `paint_surface`/`double_buffer` draw into this handle, never the
/// screen DC borrowed only to create it.
struct MemSurface {
    dc: HDC,
    bmp: HBITMAP,
    old: HGDIOBJ,
    screen: HDC,
}

impl MemSurface {
    fn new(w: i32, h: i32) -> Option<Self> {
        unsafe {
            let screen = GetDC(std::ptr::null_mut());
            if screen.is_null() {
                return None;
            }
            let dc = CreateCompatibleDC(screen);
            let bmp = CreateCompatibleBitmap(screen, w, h);
            if dc.is_null() || bmp.is_null() {
                if !dc.is_null() {
                    DeleteDC(dc);
                }
                ReleaseDC(std::ptr::null_mut(), screen);
                return None;
            }
            let old = SelectObject(dc, bmp as HGDIOBJ);
            Some(Self {
                dc,
                bmp,
                old,
                screen,
            })
        }
    }
}

impl Drop for MemSurface {
    fn drop(&mut self) {
        unsafe {
            SelectObject(self.dc, self.old);
            DeleteObject(self.bmp as HGDIOBJ);
            DeleteDC(self.dc);
            ReleaseDC(std::ptr::null_mut(), self.screen);
        }
    }
}

pub fn bench(r: &mut Runner) {
    bench_paint_surface(r, "gdi/paint_surface/462x689", 462, 689);
    bench_paint_surface(r, "gdi/paint_surface/1920x1080", 1920, 1080);
    bench_double_buffer(r, "gdi/double_buffer/1920x1080", 1920, 1080);
}

fn bench_paint_surface(r: &mut Runner, name: &str, w: i32, h: i32) {
    if !r.selected(name) {
        return;
    }
    let Some(surface) = MemSurface::new(w, h) else {
        r.skip(name, "could not create a compatible memory surface");
        return;
    };
    let tint = Tint {
        r: 32,
        g: 32,
        b: 36,
        alpha: 235,
    };
    r.bench(name, || unsafe {
        paint_surface(surface.dc, w, h, &tint, |_hdc| {});
    });
}

fn bench_double_buffer(r: &mut Runner, name: &str, w: i32, h: i32) {
    if !r.selected(name) {
        return;
    }
    let Some(surface) = MemSurface::new(w, h) else {
        r.skip(name, "could not create a compatible memory surface");
        return;
    };
    let rc = RECT {
        left: 0,
        top: 0,
        right: w,
        bottom: h,
    };
    r.bench(name, || unsafe {
        double_buffer(surface.dc, &rc, |_hdc| {});
    });
}
