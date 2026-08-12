//! The "Space N" indicator: a transient, click-through panel shown near the
//! taskbar of the monitor whose space just changed — the WinSpaces equivalent
//! of the toast Windows flashes when you switch its own virtual desktops.
//!
//! # Why this surface is layered when every other one is DWM-backdropped
//!
//! The tray flyout, Mission Control and the settings window all use the
//! `extend_frame_full` + `set_backdrop` recipe, and `docs/tray-and-menu.md` §2
//! states flatly that `WS_EX_LAYERED` must never be added to those windows —
//! layered windows and DWM system backdrops are mutually exclusive on one
//! HWND.
//!
//! This window is the deliberate exception, because it is the only surface
//! that has to *fade*. Under a DWM backdrop there is no way to fade a window:
//! dropping the painted alpha doesn't dissolve the panel, it just uncovers the
//! acrylic blur rectangle DWM is compositing underneath, so the toast would
//! "fade" into a floating blurred slab. So this surface forgoes the backdrop
//! entirely, paints an opaque panel into a premultiplied DIB, and hands the
//! whole thing to `UpdateLayeredWindow`, which gives real whole-window opacity
//! — corners and shadow-free edges included. Windows' own desktop indicator is
//! an opaque panel too, so nothing is lost visually.
//!
//! Consequences, all intentional: no DWM shadow and no
//! `DWMWA_WINDOW_CORNER_PREFERENCE`, since both are computed against the
//! rectangular window frame rather than the rounded panel inside it. The
//! corners are cut in the DIB instead, by `geometry::round_rect_coverage`.
//!
//! # Cost while idle
//!
//! Zero. No window, no DC, no timer exists until the first space switch; the
//! surface tears its GDI objects down and hides the window at the end of every
//! toast, keeping only the HWND (see `show`). Nothing ticks between switches,
//! so the daemon's fully event-driven idle contract is preserved.

pub mod geometry;

use std::cell::RefCell;
use std::ptr::{null, null_mut};

use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, SIZE, WPARAM};
use windows_sys::Win32::Graphics::Gdi::{
    CreateSolidBrush, FillRect, GdiFlush, GetStockObject, SelectObject, SetBkMode, AC_SRC_ALPHA,
    AC_SRC_OVER, BLENDFUNCTION, DEFAULT_GUI_FONT, DT_CENTER, DT_SINGLELINE, DT_VCENTER,
    FW_SEMIBOLD, HBRUSH, HDC, HFONT, HGDIOBJ, TRANSPARENT,
};
use windows_sys::Win32::System::SystemInformation::GetTickCount;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, KillTimer, SetTimer, ShowWindow, UpdateLayeredWindow, SW_HIDE,
    SW_SHOWNOACTIVATE, ULW_ALPHA, WM_TIMER, WS_EX_LAYERED, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW,
    WS_EX_TOPMOST, WS_EX_TRANSPARENT, WS_POPUP,
};
use winspaces_common::log_info;
use winspaces_core::desktop::SwitchNotice;
use winspaces_win32::display::frame_interval_ms;
use winspaces_win32::dpi::{px, scale_for_point};
use winspaces_win32::gdi::color::premultiply;
use winspaces_win32::gdi::draw::{draw_text_in, measure_text};
use winspaces_win32::gdi::font::{create_font, FACE_DISPLAY};
use winspaces_win32::gdi::guard::{DibSection, GdiObject, MemDc, ScreenDc};
use winspaces_win32::module::app_instance;
use winspaces_win32::text::encode_wide;
use winspaces_win32::window_class::register_class;

use crate::theme::{indicator_theme, load_pref, resolves_light};

const CLASS_NAME: &str = "WinSpacesIndicator";
const TIMER_FADE: usize = 1;

/// Fade in, hold, fade out — 1.27 s end to end, close to the dwell of the
/// Windows desktop toast this mirrors.
const FADE_IN_MS: u32 = 110;
const HOLD_MS: u32 = 900;
const FADE_OUT_MS: u32 = 260;
const TOTAL_MS: u32 = FADE_IN_MS + HOLD_MS + FADE_OUT_MS;

/// The live toast. At most one exists: a switch on a second monitor moves the
/// panel rather than raising a second one, which is also what makes rapid
/// switching read as one panel counting rather than a stack of them.
struct Indicator {
    hwnd: HWND,
    /// Painted panel, kept alive for the toast's duration. The fade re-blits
    /// this same bitmap with a different `SourceConstantAlpha` instead of
    /// repainting it, so a frame costs one `UpdateLayeredWindow` and no GDI.
    mem_dc: Option<MemDc>,
    dib: Option<DibSection>,
    prev_bmp: HGDIOBJ,
    pos: POINT,
    size: SIZE,
    /// `GetTickCount` at which the current toast started its fade-in.
    shown_at: u32,
    visible: bool,
    /// The label currently painted into `dib`, so a repeat switch to the same
    /// space can skip the repaint entirely.
    label: String,
    /// The theme `dib` was painted under. Part of the repaint test because the
    /// palette is re-resolved on every show (the menu's no-hooks approach), so
    /// an otherwise-identical toast still has to be redrawn after the user
    /// flips Light/Dark.
    light: bool,
}

impl Indicator {
    const fn new() -> Self {
        Self {
            hwnd: null_mut(),
            mem_dc: None,
            dib: None,
            prev_bmp: null_mut(),
            pos: POINT { x: 0, y: 0 },
            size: SIZE { cx: 0, cy: 0 },
            shown_at: 0,
            visible: false,
            label: String::new(),
            light: false,
        }
    }

    /// Restore the memory DC's original bitmap before dropping either, and
    /// forget the painted label. `DeleteObject` silently fails on a bitmap
    /// still selected into a live DC, so the order here is load-bearing.
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
        self.label.clear();
    }
}

thread_local! {
    static INDICATOR: RefCell<Indicator> = const { RefCell::new(Indicator::new()) };
}

/// The observer the bin installs on `winspaces-core`. Runs on the daemon's UI
/// thread, inside the caller's `AppState` borrow — so it must not reach back
/// into daemon state, which is why everything it needs arrives in `notice`.
pub fn on_space_switch(notice: &SwitchNotice) {
    // Mission Control already shows the active space as a highlighted card,
    // and the overlay is full-screen — a toast on top of it is redundant.
    if crate::mission_control::is_mission_control_active() {
        return;
    }
    show(notice);
}

fn show(notice: &SwitchNotice) {
    let label = format!("Space {}", notice.desk_idx + 1);

    unsafe {
        let center = POINT {
            x: (notice.work.left + notice.work.right) / 2,
            y: (notice.work.top + notice.work.bottom) / 2,
        };
        let scale = scale_for_point(center);

        let Some(screen_dc) = ScreenDc::new(null_mut()) else {
            return;
        };
        let font = GdiObject::<HFONT>::from_raw(create_font(
            FACE_DISPLAY,
            -px(scale, geometry::FONT_HEIGHT),
            FW_SEMIBOLD as i32,
        ) as HGDIOBJ);
        if font.is_null() {
            return;
        }
        let text_w = measure_text(screen_dc.handle(), font.as_raw() as HFONT, &label);
        let rect = geometry::indicator_rect(notice.work, scale, text_w);
        let width = rect.right - rect.left;
        let height = rect.bottom - rect.top;
        if width <= 0 || height <= 0 {
            return;
        }

        let light = resolves_light(load_pref());
        let theme = indicator_theme(light);

        let hwnd = INDICATOR.with(|s| s.borrow().hwnd);
        let hwnd = if hwnd.is_null() {
            let created = create_window();
            if created.is_null() {
                log_info!("Failed to create the space indicator window");
                return;
            }
            INDICATOR.with(|s| s.borrow_mut().hwnd = created);
            created
        } else {
            hwnd
        };

        let repaint = INDICATOR.with(|s| {
            let ind = s.borrow();
            ind.dib.is_none()
                || ind.label != label
                || ind.light != light
                || ind.size.cx != width
                || ind.size.cy != height
        });

        if repaint {
            INDICATOR.with(|s| s.borrow_mut().release_surface());
            let Some(mem_dc) = MemDc::new(screen_dc.handle()) else {
                return;
            };
            let Some(mut dib) = DibSection::new(screen_dc.handle(), width, height) else {
                return;
            };
            let prev_bmp = SelectObject(mem_dc.handle(), dib.as_raw());

            paint_panel(
                mem_dc.handle(),
                &mut dib,
                width,
                height,
                scale,
                font.as_raw() as HFONT,
                &theme,
                &label,
            );

            INDICATOR.with(|s| {
                let mut ind = s.borrow_mut();
                ind.mem_dc = Some(mem_dc);
                ind.dib = Some(dib);
                ind.prev_bmp = prev_bmp;
                ind.label = label;
                ind.light = light;
            });
        }

        let now = GetTickCount();
        let was_visible = INDICATOR.with(|s| {
            let mut ind = s.borrow_mut();
            ind.pos = POINT {
                x: rect.left,
                y: rect.top,
            };
            ind.size = SIZE {
                cx: width,
                cy: height,
            };
            // A switch while the panel is already up snaps it back to full
            // opacity and restarts the hold, rather than replaying the fade-in
            // — holding Alt+Left then reads as one steady panel whose number
            // changes, not a strobe.
            ind.shown_at = if ind.visible { now - FADE_IN_MS } else { now };
            let was = ind.visible;
            ind.visible = true;
            was
        });

        blit(1.0);

        if !was_visible {
            ShowWindow(hwnd, SW_SHOWNOACTIVATE);
        }
        // Re-arm every time: the interval is per-display, and the panel may
        // have just moved to a monitor running at a different refresh rate.
        SetTimer(hwnd, TIMER_FADE, frame_interval_ms(hwnd), None);
    }
}

unsafe fn create_window() -> HWND {
    register_class(CLASS_NAME, Some(indicator_wnd_proc));
    let class_name = encode_wide(CLASS_NAME);
    // WS_EX_LAYERED is the deliberate exception to the "never layer a WinSpaces
    // popup" rule — see the module doc. It is what `UpdateLayeredWindow` needs,
    // and this window sets no DWM backdrop, so the two never meet.
    //
    // WS_EX_TRANSPARENT is load-bearing rather than cosmetic: the panel floats
    // directly above the taskbar, and without it every toast would eat a click
    // aimed at whatever it covers.
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

/// Draw the panel into `dib`, then convert GDI's output into the premultiplied
/// ARGB `UpdateLayeredWindow` requires.
///
/// GDI zeroes the alpha byte of every pixel it touches and knows nothing about
/// premultiplication, so the pass at the end is where the panel actually gets
/// its shape: `round_rect_coverage` decides each pixel's alpha, and the color
/// channels are scaled to match. `gdi::surface::paint_surface` can't stand in
/// here — it promotes touched pixels to a hardcoded opaque 0xFF, which is the
/// exact byte this surface needs to vary.
#[allow(clippy::too_many_arguments)]
unsafe fn paint_panel(
    hdc: HDC,
    dib: &mut DibSection,
    width: i32,
    height: i32,
    scale: f32,
    font: HFONT,
    theme: &crate::theme::IndicatorTheme,
    label: &str,
) {
    let full = RECT {
        left: 0,
        top: 0,
        right: width,
        bottom: height,
    };

    // Fill the whole bitmap with the panel color: the rounding happens in the
    // mask pass below, not here, so the corners are simply painted and then
    // carved back out with anti-aliased coverage.
    let bg = GdiObject::<HBRUSH>::from_raw(CreateSolidBrush(theme.bg) as HGDIOBJ);
    FillRect(hdc, &full, bg.as_raw() as HBRUSH);

    SetBkMode(hdc, TRANSPARENT as i32);
    draw_text_in(
        hdc,
        font,
        theme.text,
        &full,
        label,
        DT_CENTER | DT_VCENTER | DT_SINGLELINE,
    );

    // `draw_text_in` selects the font and does not restore it, and this DC
    // outlives the caller's font handle by design (it is kept alive to be
    // re-blitted every fade frame). Deselect before that handle drops:
    // `DeleteObject` silently fails on an object still selected into a live
    // DC, which would leak one HFONT per repaint.
    SelectObject(hdc, GetStockObject(DEFAULT_GUI_FONT));

    // GDI batches; the batch must land before the bits are read back.
    GdiFlush();

    let radius = px(scale, geometry::RADIUS) as f32;
    let border_w = scale.max(1.0);
    let (br, bg_c, bb) = (
        theme.border & 0xFF,
        (theme.border >> 8) & 0xFF,
        (theme.border >> 16) & 0xFF,
    );

    let pixels = dib.pixels();
    for y in 0..height {
        for x in 0..width {
            let idx = (y * width + x) as usize;
            let pixel = pixels[idx];
            let mut b = (pixel & 0xFF) as f32;
            let mut g = ((pixel >> 8) & 0xFF) as f32;
            let mut r = ((pixel >> 16) & 0xFF) as f32;

            let outer = geometry::round_rect_coverage(x, y, width, height, radius, 0.0);
            if outer <= 0.0 {
                pixels[idx] = 0;
                continue;
            }

            // A hairline border keeps the panel readable against a wallpaper
            // that happens to match its fill. It lives in the ring between the
            // outer edge and the same shape inset by one device pixel.
            let inner =
                geometry::round_rect_coverage(x, y, width, height, radius - border_w, border_w);
            let ring = (outer - inner).clamp(0.0, 1.0);
            if ring > 0.0 {
                b += (bb as f32 - b) * ring;
                g += (bg_c as f32 - g) * ring;
                r += (br as f32 - r) * ring;
            }

            let alpha = (outer * 255.0).round() as u32;
            pixels[idx] = (alpha << 24)
                | (premultiply(r as u32, alpha) << 16)
                | (premultiply(g as u32, alpha) << 8)
                | premultiply(b as u32, alpha);
        }
    }
}

/// Push the painted bitmap to the screen at `opacity` (`0.0..=1.0`).
///
/// The per-pixel alpha carries the panel's shape and the constant alpha
/// carries the fade — Windows multiplies the two, so a frame is one call with
/// no repainting.
fn blit(opacity: f32) {
    INDICATOR.with(|s| {
        let ind = s.borrow();
        let (Some(dc), true) = (&ind.mem_dc, !ind.hwnd.is_null()) else {
            return;
        };
        let blend = BLENDFUNCTION {
            BlendOp: AC_SRC_OVER as u8,
            BlendFlags: 0,
            SourceConstantAlpha: (opacity.clamp(0.0, 1.0) * 255.0).round() as u8,
            AlphaFormat: AC_SRC_ALPHA as u8,
        };
        let src = POINT { x: 0, y: 0 };
        unsafe {
            UpdateLayeredWindow(
                ind.hwnd,
                null_mut(),
                &ind.pos,
                &ind.size,
                dc.handle(),
                &src,
                0,
                &blend,
                ULW_ALPHA,
            );
        }
    });
}

fn hide() {
    let hwnd = INDICATOR.with(|s| {
        let mut ind = s.borrow_mut();
        ind.visible = false;
        ind.release_surface();
        ind.hwnd
    });
    if !hwnd.is_null() {
        unsafe {
            KillTimer(hwnd, TIMER_FADE);
            ShowWindow(hwnd, SW_HIDE);
        }
    }
}

unsafe extern "system" fn indicator_wnd_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    if msg == WM_TIMER && wparam == TIMER_FADE {
        let elapsed = INDICATOR.with(|s| {
            let ind = s.borrow();
            if !ind.visible {
                return None;
            }
            Some(GetTickCount().wrapping_sub(ind.shown_at))
        });
        match elapsed {
            None => hide(),
            Some(e) if e >= TOTAL_MS => hide(),
            Some(e) => blit(opacity_at(e)),
        }
        return 0;
    }
    unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
}

/// Opacity for a toast `elapsed` ms into its life.
fn opacity_at(elapsed: u32) -> f32 {
    if elapsed < FADE_IN_MS {
        geometry::smoothstep(elapsed as f32 / FADE_IN_MS as f32)
    } else if elapsed < FADE_IN_MS + HOLD_MS {
        1.0
    } else if elapsed < TOTAL_MS {
        let t = (elapsed - FADE_IN_MS - HOLD_MS) as f32 / FADE_OUT_MS as f32;
        geometry::smoothstep(1.0 - t)
    } else {
        0.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn opacity_curve_covers_every_phase() {
        assert_eq!(opacity_at(0), 0.0);
        assert!(opacity_at(FADE_IN_MS / 2) > 0.0);
        assert!(opacity_at(FADE_IN_MS / 2) < 1.0);
        assert_eq!(opacity_at(FADE_IN_MS), 1.0);
        assert_eq!(opacity_at(FADE_IN_MS + HOLD_MS - 1), 1.0);
        assert!(opacity_at(FADE_IN_MS + HOLD_MS + FADE_OUT_MS / 2) < 1.0);
        assert_eq!(opacity_at(TOTAL_MS), 0.0);
        assert_eq!(opacity_at(TOTAL_MS * 4), 0.0);
    }

    #[test]
    fn fade_out_is_monotonically_decreasing() {
        let start = FADE_IN_MS + HOLD_MS;
        let mut prev = 1.0;
        for step in 0..=FADE_OUT_MS / 10 {
            let o = opacity_at(start + step * 10);
            assert!(o <= prev, "opacity must never rise during fade-out");
            prev = o;
        }
    }
}
