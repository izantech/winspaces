//! Ghost preview for the Shift+drag split toggle: a translucent accent-tinted
//! rendering of the tile layout the drop would produce, shown while `Shift` is
//! held during a window drag on a tiled space.
//!
//! Same surface recipe as the space indicator, and for the same reason — this
//! window has to be see-through, which a DWM-backdropped popup cannot be (see
//! `space_indicator`'s module doc). The shape is painted directly into a
//! premultiplied ARGB DIB and handed to `UpdateLayeredWindow`; per-pixel alpha
//! carries both the 25% fill and the opaque border, so no GDI drawing runs at
//! all.
//!
//! # Cost while idle
//!
//! Zero, on the space indicator's contract: no window, DC or bitmap exists
//! until the first preview, and `hide_preview` tears down everything but the
//! HWND. The bin's drag poll timer — not this module — decides when frames
//! happen, and that timer only exists while a tiled drag is in flight.

use std::cell::RefCell;
use std::ptr::{null, null_mut};

use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, SIZE, WPARAM};
use windows_sys::Win32::Graphics::Gdi::{
    SelectObject, AC_SRC_ALPHA, AC_SRC_OVER, BLENDFUNCTION, HGDIOBJ,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, ShowWindow, UpdateLayeredWindow, SW_HIDE, SW_SHOWNOACTIVATE,
    ULW_ALPHA, WS_EX_LAYERED, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_EX_TRANSPARENT,
    WS_POPUP,
};
use winspaces_common::WindowRect;
use winspaces_win32::dpi::{px, scale_for_point};
use winspaces_win32::gdi::color::premultiply;
use winspaces_win32::gdi::guard::{DibSection, MemDc, ScreenDc};
use winspaces_win32::module::app_instance;
use winspaces_win32::text::encode_wide;
use winspaces_win32::window_class::register_class;

use crate::space_indicator::geometry::round_rect_coverage;
use crate::theme::preview_accent;

const CLASS_NAME: &str = "WinSpacesTilingPreview";

/// Tile corner radius at 96 dpi — matches the rounding DWM applies to the
/// real windows the preview stands in for.
const RADIUS: i32 = 8;
/// Border width at 96 dpi.
const BORDER_W: i32 = 2;
/// Interior tint alpha (~25%).
const FILL_ALPHA: f32 = 64.0;
/// Border alpha — near-opaque so the split line reads over any wallpaper.
const BORDER_ALPHA: f32 = 230.0;

struct Preview {
    hwnd: HWND,
    mem_dc: Option<MemDc>,
    dib: Option<DibSection>,
    prev_bmp: HGDIOBJ,
    visible: bool,
    /// What the current DIB shows, so a tick with an unchanged prediction is
    /// a no-op instead of a 2M-pixel repaint.
    painted: Option<(RECT, Vec<WindowRect>)>,
}

impl Preview {
    const fn new() -> Self {
        Self {
            hwnd: null_mut(),
            mem_dc: None,
            dib: None,
            prev_bmp: null_mut(),
            visible: false,
            painted: None,
        }
    }

    /// Restore the memory DC's original bitmap before dropping either — same
    /// load-bearing order as the space indicator's `release_surface`.
    fn release_surface(&mut self) {
        if let Some(dc) = &self.mem_dc {
            if !self.prev_bmp.is_null() {
                unsafe {
                    SelectObject(dc.handle(), self.prev_bmp);
                }
            }
        }
        self.prev_bmp = null_mut();
        self.dib = None;
        self.mem_dc = None;
        self.painted = None;
    }
}

thread_local! {
    static PREVIEW: RefCell<Preview> = const { RefCell::new(Preview::new()) };
}

/// Show (or update) the ghost preview: `rects` are the predicted tile frames
/// in screen coordinates, `work` the monitor work rect the window spans.
/// Idempotent — an unchanged prediction repaints nothing.
pub fn show_preview(work: &RECT, rects: &[WindowRect]) {
    let width = work.right - work.left;
    let height = work.bottom - work.top;
    if width <= 0 || height <= 0 || rects.is_empty() {
        return;
    }

    let unchanged = PREVIEW.with(|s| {
        let p = s.borrow();
        p.visible
            && p.painted.as_ref().is_some_and(|(w, r)| {
                w.left == work.left
                    && w.top == work.top
                    && w.right == work.right
                    && w.bottom == work.bottom
                    && r == rects
            })
    });
    if unchanged {
        return;
    }

    unsafe {
        let hwnd = PREVIEW.with(|s| s.borrow().hwnd);
        let hwnd = if hwnd.is_null() {
            let created = create_window();
            if created.is_null() {
                return;
            }
            PREVIEW.with(|s| s.borrow_mut().hwnd = created);
            created
        } else {
            hwnd
        };

        let Some(screen_dc) = ScreenDc::new(null_mut()) else {
            return;
        };
        PREVIEW.with(|s| s.borrow_mut().release_surface());
        let Some(mem_dc) = MemDc::new(screen_dc.handle()) else {
            return;
        };
        let Some(mut dib) = DibSection::new(screen_dc.handle(), width, height) else {
            return;
        };
        let prev_bmp = SelectObject(mem_dc.handle(), dib.as_raw());

        let center = POINT {
            x: (work.left + work.right) / 2,
            y: (work.top + work.bottom) / 2,
        };
        paint_rects(&mut dib, work, rects, scale_for_point(center));

        let was_visible = PREVIEW.with(|s| {
            let mut p = s.borrow_mut();
            p.mem_dc = Some(mem_dc);
            p.dib = Some(dib);
            p.prev_bmp = prev_bmp;
            p.painted = Some((*work, rects.to_vec()));
            let was = p.visible;
            p.visible = true;
            was
        });

        // No fade: drag feedback has to track the modifier instantly, so the
        // frame goes up at full constant alpha and the per-pixel channel does
        // all the shaping.
        let blend = BLENDFUNCTION {
            BlendOp: AC_SRC_OVER as u8,
            BlendFlags: 0,
            SourceConstantAlpha: 255,
            AlphaFormat: AC_SRC_ALPHA as u8,
        };
        let pos = POINT {
            x: work.left,
            y: work.top,
        };
        let size = SIZE {
            cx: width,
            cy: height,
        };
        let src = POINT { x: 0, y: 0 };
        PREVIEW.with(|s| {
            let p = s.borrow();
            if let Some(dc) = &p.mem_dc {
                UpdateLayeredWindow(
                    p.hwnd,
                    null_mut(),
                    &pos,
                    &size,
                    dc.handle(),
                    &src,
                    0,
                    &blend,
                    ULW_ALPHA,
                );
            }
        });

        if !was_visible {
            ShowWindow(hwnd, SW_SHOWNOACTIVATE);
        }
    }
}

/// Hide the preview and drop its surface. Safe when nothing is shown.
pub fn hide_preview() {
    let hwnd = PREVIEW.with(|s| {
        let mut p = s.borrow_mut();
        if !p.visible && p.dib.is_none() {
            return null_mut();
        }
        p.visible = false;
        p.release_surface();
        p.hwnd
    });
    if !hwnd.is_null() {
        unsafe {
            ShowWindow(hwnd, SW_HIDE);
        }
    }
}

unsafe fn create_window() -> HWND {
    register_class(CLASS_NAME, Some(preview_wnd_proc));
    let class_name = encode_wide(CLASS_NAME);
    // WS_EX_TRANSPARENT is load-bearing: the preview sits over the very tiles
    // the user is dragging across, and must never eat the drop.
    CreateWindowExW(
        WS_EX_LAYERED | WS_EX_TRANSPARENT | WS_EX_TOOLWINDOW | WS_EX_TOPMOST | WS_EX_NOACTIVATE,
        class_name.as_ptr(),
        null(),
        WS_POPUP,
        0,
        0,
        0,
        0,
        null_mut(),
        null_mut(),
        app_instance(),
        null_mut(),
    )
}

/// Write the predicted tiles straight into the zero-initialized (transparent)
/// DIB: accent fill at `FILL_ALPHA`, a `BORDER_W` accent ring at
/// `BORDER_ALPHA`, corners rounded by signed-distance coverage. Tiles never
/// overlap, so each rect owns its pixels outright.
fn paint_rects(dib: &mut DibSection, work: &RECT, rects: &[WindowRect], scale: f32) {
    let width = dib.width;
    let height = dib.height;
    let radius = px(scale, RADIUS) as f32;
    let border_w = (px(scale, BORDER_W) as f32).max(1.0);

    let accent = preview_accent();
    let (ar, ag, ab) = (accent & 0xFF, (accent >> 8) & 0xFF, (accent >> 16) & 0xFF);

    let pixels = dib.pixels();
    for r in rects {
        let left = (r.left - work.left).clamp(0, width);
        let top = (r.top - work.top).clamp(0, height);
        let right = (r.right - work.left).clamp(0, width);
        let bottom = (r.bottom - work.top).clamp(0, height);
        let rw = right - left;
        let rh = bottom - top;
        if rw <= 0 || rh <= 0 {
            continue;
        }

        // Coverage varies only near the tile's edge; interior pixels take the
        // flat fill without the per-pixel sqrt (same banding as the space
        // indicator's mask pass).
        let band = (radius.ceil().max(border_w.ceil()) as i32 + 1).min(rw.min(rh));
        let fill_a = FILL_ALPHA.round() as u32;
        let fill_px = (fill_a << 24)
            | (premultiply(ar, fill_a) << 16)
            | (premultiply(ag, fill_a) << 8)
            | premultiply(ab, fill_a);

        for ry in 0..rh {
            let edge_row = ry < band || ry >= rh - band;
            let row_base = (top + ry) * width + left;
            for rx in 0..rw {
                let idx = (row_base + rx) as usize;
                if !edge_row && rx >= band && rx < rw - band {
                    pixels[idx] = fill_px;
                    continue;
                }
                let outer = round_rect_coverage(rx, ry, rw, rh, radius, 0.0);
                if outer <= 0.0 {
                    continue;
                }
                let inner = round_rect_coverage(rx, ry, rw, rh, radius - border_w, border_w);
                let ring = (outer - inner).clamp(0.0, 1.0);
                let a = (inner.min(outer) * FILL_ALPHA + ring * BORDER_ALPHA)
                    .clamp(0.0, 255.0)
                    .round() as u32;
                pixels[idx] = (a << 24)
                    | (premultiply(ar, a) << 16)
                    | (premultiply(ag, a) << 8)
                    | premultiply(ab, a);
            }
        }
    }
}

unsafe extern "system" fn preview_wnd_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
}
