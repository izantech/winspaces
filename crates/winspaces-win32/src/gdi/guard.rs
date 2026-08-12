//! RAII wrappers around manually-paired Win32 GDI lifetimes.
//!
//! Shipped in batch 5 of the crate reorganization; **not adopted by any
//! call site yet** — that's batch 11, gated on `GetGuiResources` counts
//! measured before and after, since a dropped `DeleteObject` leaks silently
//! for hours rather than failing a test. Nothing here changes behavior; it
//! only makes the existing create/delete and select/restore pairs
//! impossible to get wrong once something adopts them.

use std::marker::PhantomData;
use std::ptr::null_mut;

use windows_sys::Win32::Foundation::HWND;
use windows_sys::Win32::Graphics::Gdi::{
    CreateCompatibleDC, CreateDIBSection, DeleteDC, DeleteObject, GetDC, ReleaseDC, SelectObject,
    BITMAPINFO, BITMAPINFOHEADER, BI_RGB, DIB_RGB_COLORS, HDC, HGDIOBJ,
};

/// A memory device context created with `CreateCompatibleDC`, deleted with
/// `DeleteDC` on drop.
pub struct MemDc(HDC);

impl MemDc {
    /// # Safety
    /// `compatible_with` must be null or a valid device context.
    pub unsafe fn new(compatible_with: HDC) -> Option<Self> {
        let dc = unsafe { CreateCompatibleDC(compatible_with) };
        if dc.is_null() {
            None
        } else {
            Some(Self(dc))
        }
    }

    pub fn handle(&self) -> HDC {
        self.0
    }
}

impl Drop for MemDc {
    fn drop(&mut self) {
        unsafe {
            DeleteDC(self.0);
        }
    }
}

/// A window device context obtained with `GetDC`, released with
/// `ReleaseDC` on drop. `GetDC(null)` (the screen DC) works the same way.
pub struct ScreenDc {
    hwnd: HWND,
    hdc: HDC,
}

impl ScreenDc {
    /// # Safety
    /// `hwnd` must be null (the screen DC) or a valid window handle.
    pub unsafe fn new(hwnd: HWND) -> Option<Self> {
        let hdc = unsafe { GetDC(hwnd) };
        if hdc.is_null() {
            None
        } else {
            Some(Self { hwnd, hdc })
        }
    }

    pub fn handle(&self) -> HDC {
        self.hdc
    }
}

impl Drop for ScreenDc {
    fn drop(&mut self) {
        unsafe {
            ReleaseDC(self.hwnd, self.hdc);
        }
    }
}

/// A GDI object (brush, pen, font, bitmap, ...) deleted with `DeleteObject`
/// on drop. `T` (typically an `HFONT`/`HBRUSH`/`HPEN`/`HBITMAP` marker) just
/// distinguishes handle kinds at the type level; the handle itself is
/// stored as the untyped `HGDIOBJ` `DeleteObject` expects.
pub struct GdiObject<T> {
    handle: HGDIOBJ,
    _marker: PhantomData<fn() -> T>,
}

impl<T> GdiObject<T> {
    /// Wrap an already-created handle.
    ///
    /// # Safety
    /// `handle` must be a valid GDI object handle not owned (i.e. not
    /// subject to deletion) by anything else.
    pub unsafe fn from_raw(handle: HGDIOBJ) -> Self {
        Self {
            handle,
            _marker: PhantomData,
        }
    }

    pub fn as_raw(&self) -> HGDIOBJ {
        self.handle
    }

    pub fn is_null(&self) -> bool {
        self.handle.is_null()
    }
}

impl<T> Drop for GdiObject<T> {
    fn drop(&mut self) {
        if !self.handle.is_null() {
            unsafe {
                DeleteObject(self.handle);
            }
        }
    }
}

/// Selects a GDI object into a DC and restores the DC's previously
/// selected object on drop. Structurally prevents the "select, forget to
/// restore before delete" pattern `DeleteObject` silently fails on for an
/// object still selected into a DC.
pub struct SelectGuard<'a> {
    hdc: HDC,
    prev: HGDIOBJ,
    _marker: PhantomData<&'a ()>,
}

impl<'a> SelectGuard<'a> {
    /// # Safety
    /// `hdc` must be a valid device context; `obj` must be a valid, live
    /// GDI object selectable into it, and must outlive this guard.
    pub unsafe fn new(hdc: HDC, obj: HGDIOBJ) -> Self {
        let prev = unsafe { SelectObject(hdc, obj) };
        Self {
            hdc,
            prev,
            _marker: PhantomData,
        }
    }
}

impl Drop for SelectGuard<'_> {
    fn drop(&mut self) {
        unsafe {
            SelectObject(self.hdc, self.prev);
        }
    }
}

/// A `CreateDIBSection` bitmap plus its writable pixel buffer, deleted with
/// `DeleteObject` on drop. Top-down 32bpp `BI_RGB` — the layout
/// `gdi::surface::paint_surface` and the menu/tray premultiplied-alpha
/// recipes all rely on. `pixels()` is only valid for this object's
/// lifetime, and only sound to read/write while the bitmap isn't selected
/// into a DC that outlives it.
pub struct DibSection {
    bitmap: HGDIOBJ,
    bits: *mut u8,
    pub width: i32,
    pub height: i32,
}

impl DibSection {
    /// # Safety
    /// `target` must be a valid device context.
    pub unsafe fn new(target: HDC, width: i32, height: i32) -> Option<Self> {
        if width <= 0 || height <= 0 {
            return None;
        }
        let mut bmi: BITMAPINFO = unsafe { std::mem::zeroed() };
        bmi.bmiHeader.biSize = std::mem::size_of::<BITMAPINFOHEADER>() as u32;
        bmi.bmiHeader.biWidth = width;
        bmi.bmiHeader.biHeight = -height; // top-down
        bmi.bmiHeader.biPlanes = 1;
        bmi.bmiHeader.biBitCount = 32;
        bmi.bmiHeader.biCompression = BI_RGB;
        let mut bits: *mut u8 = null_mut();
        let bitmap = unsafe {
            CreateDIBSection(
                target,
                &bmi,
                DIB_RGB_COLORS,
                &mut bits as *mut *mut u8 as _,
                null_mut(),
                0,
            )
        };
        if bitmap.is_null() {
            return None;
        }
        Some(Self {
            bitmap,
            bits,
            width,
            height,
        })
    }

    pub fn as_raw(&self) -> HGDIOBJ {
        self.bitmap
    }

    /// The pixel buffer as top-down 32bpp words (one `u32` per pixel),
    /// `width * height` long.
    pub fn pixels(&mut self) -> &mut [u32] {
        unsafe {
            std::slice::from_raw_parts_mut(
                self.bits as *mut u32,
                (self.width * self.height) as usize,
            )
        }
    }
}

impl Drop for DibSection {
    fn drop(&mut self) {
        if !self.bitmap.is_null() {
            unsafe {
                DeleteObject(self.bitmap);
            }
        }
    }
}
