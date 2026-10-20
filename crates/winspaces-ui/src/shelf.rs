//! The Shelf strip: one window per display along the edge Shelf reserves,
//! holding a card per hidden window. A card shows a live DWM thumbnail of its
//! window, or its icon and title when the window was hidden by a backend that
//! leaves no surface to thumbnail (forced-minimize, `SW_HIDE`). Clicking a
//! card swaps it onto the stage; dragging one along the strip reorders it.
//! The daemon owns the roles (`winspaces-core`'s `spaces::shelf`); this module
//! only paints what `sync` hands it and reports gestures through `ShelfHost`.

use std::cell::RefCell;
use std::ptr::null_mut;
use std::sync::OnceLock;
use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, SIZE, WPARAM};
use windows_sys::Win32::Graphics::Dwm::{
    DwmQueryThumbnailSourceSize, DwmRegisterThumbnail, DwmUnregisterThumbnail,
    DwmUpdateThumbnailProperties, DWM_THUMBNAIL_PROPERTIES, DWM_TNP_OPACITY,
    DWM_TNP_RECTDESTINATION, DWM_TNP_SOURCECLIENTAREAONLY, DWM_TNP_VISIBLE,
};
use windows_sys::Win32::Graphics::Gdi::{
    BeginPaint, CreateSolidBrush, DeleteObject, EndPaint, FillRect, InvalidateRect, SetBkMode,
    DT_CENTER, DT_END_ELLIPSIS, DT_SINGLELINE, DT_VCENTER, HBRUSH, HFONT, PAINTSTRUCT, TRANSPARENT,
};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{ReleaseCapture, SetCapture};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, DrawIconEx, GetClientRect, GetSystemMetrics,
    GetWindowTextW, SetWindowPos, ShowWindow, DI_NORMAL, HICON, HWND_TOPMOST, MA_NOACTIVATE,
    SM_CXDRAG, SM_CYDRAG, SWP_NOACTIVATE, SW_HIDE, SW_SHOWNOACTIVATE, WM_CAPTURECHANGED,
    WM_ERASEBKGND, WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MOUSEACTIVATE, WM_MOUSEMOVE, WM_PAINT,
    WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_POPUP,
};
use winspaces_common::i18n::t;
use winspaces_common::Msg;
use winspaces_win32::dpi;
use winspaces_win32::dwm;
use winspaces_win32::gdi::color::rgb;
use winspaces_win32::gdi::draw::{draw_text_in, fill_round};
use winspaces_win32::gdi::font::{create_font, FACE_TEXT};
use winspaces_win32::gdi::surface::double_buffer;
use winspaces_win32::module::app_instance;
use winspaces_win32::text::encode_wide;

use crate::window_icon::get_window_icon;

const CLASS_NAME: &str = "WinSpacesShelfStrip";

/// What the daemon wants one display's strip to show. `rect` is in screen
/// coordinates; a strip taller than wide stacks its cards vertically.
pub struct StripModel {
    pub rect: RECT,
    /// Each card's window, most recent first, and whether its thumbnail is live.
    pub cards: Vec<(HWND, bool)>,
}

/// Gestures the strip reports back. Called after the strip's state borrow is
/// released: the host re-enters `sync`.
pub struct ShelfHost {
    pub promote: fn(hwnd: HWND),
    pub reorder: fn(mon: usize, order: Vec<HWND>),
}

static HOST: OnceLock<&'static ShelfHost> = OnceLock::new();

pub fn install_host(host: &'static ShelfHost) {
    let _ = HOST.set(host);
}

struct Card {
    hwnd: HWND,
    rect: RECT,
    thumb: isize,
    icon: HICON,
    title: String,
}

struct Press {
    card: usize,
    start: POINT,
    dragging: bool,
    /// Offset of the dragged card along the strip axis.
    offset: i32,
}

struct Strip {
    hwnd: HWND,
    vertical: bool,
    scale: f32,
    font: HFONT,
    cards: Vec<Card>,
    press: Option<Press>,
}

impl Strip {
    fn release_cards(&mut self) {
        for card in self.cards.drain(..) {
            if card.thumb != 0 {
                unsafe {
                    DwmUnregisterThumbnail(card.thumb);
                }
            }
        }
    }

    fn hide(&mut self) {
        self.release_cards();
        self.press = None;
        unsafe {
            ShowWindow(self.hwnd, SW_HIDE);
        }
    }
}

thread_local! {
    static STRIPS: RefCell<Vec<Strip>> = const { RefCell::new(Vec::new()) };
}

/// Show one strip per display with a model and hide the others. Index `i` of
/// `models` is display `i`. `above` is a window the strips must stay under
/// (an open Overview), or null.
#[allow(clippy::not_unsafe_ptr_arg_deref)]
pub fn sync(models: &[Option<StripModel>], above: HWND) {
    STRIPS.with(|s| {
        let Ok(mut strips) = s.try_borrow_mut() else {
            return;
        };
        while strips.len() > models.len() {
            if let Some(mut strip) = strips.pop() {
                strip.release_cards();
                unsafe {
                    DeleteObject(strip.font as _);
                    DestroyWindow(strip.hwnd);
                }
            }
        }
        for (i, model) in models.iter().enumerate() {
            if strips.len() <= i {
                let Some(strip) = (unsafe { create_strip() }) else {
                    return;
                };
                strips.push(strip);
            }
            let strip = &mut strips[i];
            match model {
                Some(model) if !model.cards.is_empty() => unsafe { show(strip, model, above) },
                _ => strip.hide(),
            }
        }
    });
}

unsafe fn create_strip() -> Option<Strip> {
    winspaces_win32::window_class::register_class(CLASS_NAME, Some(strip_wnd_proc));
    let class = encode_wide(CLASS_NAME);
    let hwnd = CreateWindowExW(
        WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
        class.as_ptr(),
        std::ptr::null(),
        WS_POPUP,
        0,
        0,
        0,
        0,
        null_mut(),
        null_mut(),
        app_instance(),
        null_mut(),
    );
    if hwnd.is_null() {
        return None;
    }
    dwm::set_dark_mode(hwnd, true);
    dwm::set_round_corners(hwnd);
    dwm::set_backdrop(hwnd, dwm::Backdrop::Acrylic);
    Some(Strip {
        hwnd,
        vertical: true,
        scale: 0.0,
        font: null_mut(),
        cards: Vec::new(),
        press: None,
    })
}

unsafe fn show(strip: &mut Strip, model: &StripModel, above: HWND) {
    let scale = dpi::scale_for_window(strip.hwnd);
    let inset = dpi::px(scale, 8);
    let r = &model.rect;
    let (x, y) = (r.left + inset, r.top + inset);
    let (w, h) = (r.right - r.left - 2 * inset, r.bottom - r.top - 2 * inset);
    let insert_after = if above.is_null() { HWND_TOPMOST } else { above };
    SetWindowPos(strip.hwnd, insert_after, x, y, w, h, SWP_NOACTIVATE);
    ShowWindow(strip.hwnd, SW_SHOWNOACTIVATE);

    let scale = dpi::scale_for_window(strip.hwnd);
    if scale != strip.scale || strip.font.is_null() {
        if !strip.font.is_null() {
            DeleteObject(strip.font as _);
        }
        strip.font = create_font(FACE_TEXT, dpi::px(scale, 12), 400);
        strip.scale = scale;
    }
    strip.vertical = h > w;
    strip.press = None;

    let rects = card_rects(w, h, strip.vertical, model.cards.len(), scale);
    let mut kept: Vec<(HWND, isize)> = strip.cards.drain(..).map(|c| (c.hwnd, c.thumb)).collect();
    for (&(hwnd, live), rect) in model.cards.iter().zip(rects) {
        let reused = kept
            .iter()
            .position(|&(h, _)| h == hwnd)
            .map(|i| kept.remove(i).1);
        let mut thumb = reused.unwrap_or(0);
        if !live && thumb != 0 {
            DwmUnregisterThumbnail(thumb);
            thumb = 0;
        }
        if live && thumb == 0 && DwmRegisterThumbnail(strip.hwnd, hwnd, &mut thumb) != 0 {
            thumb = 0;
        }
        let card = Card {
            hwnd,
            rect,
            thumb,
            icon: if thumb == 0 {
                get_window_icon(hwnd)
            } else {
                null_mut()
            },
            title: window_title(hwnd),
        };
        place_thumbnail(&card, 0, strip.vertical, scale);
        strip.cards.push(card);
    }
    for (_, thumb) in kept {
        if thumb != 0 {
            DwmUnregisterThumbnail(thumb);
        }
    }
    InvalidateRect(strip.hwnd, std::ptr::null(), 1);
}

unsafe fn window_title(hwnd: HWND) -> String {
    let mut buf = [0u16; 256];
    let len = GetWindowTextW(hwnd, buf.as_mut_ptr(), buf.len() as i32);
    if len > 0 {
        String::from_utf16_lossy(&buf[..len as usize])
    } else {
        t(Msg::OverviewUntitled).to_string()
    }
}

/// Fit the card's thumbnail inside its rect, shifted `offset` along the strip.
unsafe fn place_thumbnail(card: &Card, offset: i32, vertical: bool, scale: f32) {
    if card.thumb == 0 {
        return;
    }
    let pad = dpi::px(scale, 6);
    let r = shifted(&card.rect, offset, vertical);
    let (bw, bh) = (r.right - r.left - 2 * pad, r.bottom - r.top - 2 * pad);
    let mut src = SIZE { cx: 0, cy: 0 };
    let (tw, th) =
        if DwmQueryThumbnailSourceSize(card.thumb, &mut src) == 0 && src.cx > 0 && src.cy > 0 {
            let k = (bw as f32 / src.cx as f32).min(bh as f32 / src.cy as f32);
            ((src.cx as f32 * k) as i32, (src.cy as f32 * k) as i32)
        } else {
            (bw, bh)
        };
    let left = r.left + pad + (bw - tw) / 2;
    let top = r.top + pad + (bh - th) / 2;
    let mut props: DWM_THUMBNAIL_PROPERTIES = std::mem::zeroed();
    props.dwFlags =
        DWM_TNP_RECTDESTINATION | DWM_TNP_VISIBLE | DWM_TNP_OPACITY | DWM_TNP_SOURCECLIENTAREAONLY;
    props.rcDestination = RECT {
        left,
        top,
        right: left + tw,
        bottom: top + th,
    };
    props.fVisible = 1;
    props.opacity = 255;
    props.fSourceClientAreaOnly = 0;
    DwmUpdateThumbnailProperties(card.thumb, &props);
}

fn shifted(r: &RECT, offset: i32, vertical: bool) -> RECT {
    if vertical {
        RECT {
            top: r.top + offset,
            bottom: r.bottom + offset,
            ..*r
        }
    } else {
        RECT {
            left: r.left + offset,
            right: r.right + offset,
            ..*r
        }
    }
}

/// Card rects along a `w` x `h` strip: as large as the strip's thickness
/// allows, shrunk to fit them all, centred along the axis.
// ponytail: cards that still do not fit at the minimum size are dropped from
// the strip (Overview and the taskbar still reach them); scroll if it matters.
fn card_rects(w: i32, h: i32, vertical: bool, n: usize, scale: f32) -> Vec<RECT> {
    let px = |v| dpi::px(scale, v);
    let (pad, gap, min) = (px(10), px(10), px(48));
    let (along, across) = if vertical { (h, w) } else { (w, h) };
    let thick = across - 2 * pad;
    let room = along - 2 * pad;
    if n == 0 || thick <= 0 || room <= 0 {
        return Vec::new();
    }
    let mut len = if vertical {
        thick * 3 / 4
    } else {
        thick * 4 / 3
    };
    let mut count = n as i32;
    if count * len + (count - 1) * gap > room {
        len = ((room - (count - 1) * gap) / count).max(min);
        count = ((room + gap) / (len + gap)).min(count);
    }
    let total = count * len + (count - 1).max(0) * gap;
    let start = pad + (room - total) / 2;
    (0..count)
        .map(|i| {
            let a = start + i * (len + gap);
            if vertical {
                RECT {
                    left: pad,
                    top: a,
                    right: pad + thick,
                    bottom: a + len,
                }
            } else {
                RECT {
                    left: a,
                    top: pad,
                    right: a + len,
                    bottom: pad + thick,
                }
            }
        })
        .collect()
}

/// Where card `from` lands when dropped with its centre at `pos` along the
/// axis: past every other card whose centre it crossed.
fn drop_slot(from: usize, pos: i32, rects: &[RECT], vertical: bool) -> usize {
    let centre = |r: &RECT| {
        if vertical {
            (r.top + r.bottom) / 2
        } else {
            (r.left + r.right) / 2
        }
    };
    rects
        .iter()
        .enumerate()
        .filter(|&(i, r)| i != from && centre(r) < pos)
        .count()
}

fn axis(pt: POINT, vertical: bool) -> i32 {
    if vertical {
        pt.y
    } else {
        pt.x
    }
}

fn point(lparam: LPARAM) -> POINT {
    POINT {
        x: (lparam & 0xFFFF) as i16 as i32,
        y: ((lparam >> 16) & 0xFFFF) as i16 as i32,
    }
}

fn contains(r: &RECT, pt: POINT) -> bool {
    pt.x >= r.left && pt.x < r.right && pt.y >= r.top && pt.y < r.bottom
}

/// What a button release asks the host to do, decided inside the borrow.
enum Release {
    Promote(HWND),
    Reorder(usize, Vec<HWND>),
}

unsafe extern "system" fn strip_wnd_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match msg {
        WM_MOUSEACTIVATE => MA_NOACTIVATE as LRESULT,
        WM_ERASEBKGND => 1,
        WM_PAINT => {
            let mut ps: PAINTSTRUCT = std::mem::zeroed();
            let hdc = BeginPaint(hwnd, &mut ps);
            let mut rc: RECT = std::mem::zeroed();
            GetClientRect(hwnd, &mut rc);
            double_buffer(hdc, &rc, |dc| paint(dc, hwnd, &rc));
            EndPaint(hwnd, &ps);
            0
        }
        WM_LBUTTONDOWN => {
            let pt = point(lparam);
            let pressed = with_strip(hwnd, |strip, _| {
                let card = strip.cards.iter().position(|c| contains(&c.rect, pt))?;
                strip.press = Some(Press {
                    card,
                    start: pt,
                    dragging: false,
                    offset: 0,
                });
                Some(())
            })
            .flatten();
            if pressed.is_some() {
                SetCapture(hwnd);
            }
            0
        }
        WM_MOUSEMOVE => {
            let pt = point(lparam);
            with_strip(hwnd, |strip, _| {
                let vertical = strip.vertical;
                let scale = strip.scale;
                let Some(press) = strip.press.as_mut() else {
                    return;
                };
                let delta = axis(pt, vertical) - axis(press.start, vertical);
                let threshold =
                    GetSystemMetrics(if vertical { SM_CYDRAG } else { SM_CXDRAG }).max(4);
                if !press.dragging && delta.abs() <= threshold {
                    return;
                }
                press.dragging = true;
                press.offset = delta;
                let card = press.card;
                place_thumbnail(&strip.cards[card], delta, vertical, scale);
                InvalidateRect(hwnd, std::ptr::null(), 0);
            });
            0
        }
        WM_LBUTTONUP => {
            let release = with_strip(hwnd, |strip, mon| {
                let press = strip.press.take()?;
                let card = &strip.cards[press.card];
                if !press.dragging {
                    return Some(Release::Promote(card.hwnd));
                }
                let r = shifted(&card.rect, press.offset, strip.vertical);
                let pos = if strip.vertical {
                    (r.top + r.bottom) / 2
                } else {
                    (r.left + r.right) / 2
                };
                let rects: Vec<RECT> = strip.cards.iter().map(|c| c.rect).collect();
                let slot = drop_slot(press.card, pos, &rects, strip.vertical);
                let mut order: Vec<HWND> = strip.cards.iter().map(|c| c.hwnd).collect();
                let moved = order.remove(press.card);
                order.insert(slot, moved);
                Some(Release::Reorder(mon, order))
            })
            .flatten();
            ReleaseCapture();
            match (release, HOST.get()) {
                (Some(Release::Promote(h)), Some(host)) => (host.promote)(h),
                (Some(Release::Reorder(mon, order)), Some(host)) => (host.reorder)(mon, order),
                _ => {}
            }
            0
        }
        WM_CAPTURECHANGED => {
            // Arrives re-entrantly from `ReleaseCapture` or a `sync` that hides
            // the strip mid-press; a press still pending here was interrupted.
            STRIPS.with(|s| {
                if let Ok(mut strips) = s.try_borrow_mut() {
                    if let Some(strip) = strips.iter_mut().find(|st| st.hwnd == hwnd) {
                        if let Some(press) = strip.press.take() {
                            place_thumbnail(
                                &strip.cards[press.card],
                                0,
                                strip.vertical,
                                strip.scale,
                            );
                            InvalidateRect(hwnd, std::ptr::null(), 0);
                        }
                    }
                }
            });
            0
        }
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

/// Run `f` on the strip owning `hwnd`, with its display index. `None` when the
/// state is borrowed (a re-entrant message) or the window is unknown.
fn with_strip<R>(hwnd: HWND, f: impl FnOnce(&mut Strip, usize) -> R) -> Option<R> {
    STRIPS.with(|s| {
        let mut strips = s.try_borrow_mut().ok()?;
        let mon = strips.iter().position(|st| st.hwnd == hwnd)?;
        Some(f(&mut strips[mon], mon))
    })
}

unsafe fn paint(hdc: windows_sys::Win32::Graphics::Gdi::HDC, hwnd: HWND, rc: &RECT) {
    let bg = CreateSolidBrush(rgb(0x14, 0x14, 0x18));
    FillRect(hdc, rc, bg as HBRUSH);
    DeleteObject(bg as _);
    SetBkMode(hdc, TRANSPARENT as i32);
    STRIPS.with(|s| {
        let Ok(strips) = s.try_borrow() else {
            return;
        };
        let Some(strip) = strips.iter().find(|st| st.hwnd == hwnd) else {
            return;
        };
        let px = |v| dpi::px(strip.scale, v);
        for (i, card) in strip.cards.iter().enumerate() {
            let offset = match &strip.press {
                Some(p) if p.card == i && p.dragging => p.offset,
                _ => 0,
            };
            let r = shifted(&card.rect, offset, strip.vertical);
            fill_round(hdc, &r, px(8), rgb(0x24, 0x24, 0x2B), rgb(0x34, 0x34, 0x3D));
            if card.thumb != 0 {
                continue;
            }
            // No live surface: the window's icon over its title.
            let icon = px(32);
            let cx = (r.left + r.right) / 2;
            let cy = (r.top + r.bottom) / 2;
            if !card.icon.is_null() {
                DrawIconEx(
                    hdc,
                    cx - icon / 2,
                    cy - icon,
                    card.icon,
                    icon,
                    icon,
                    0,
                    null_mut(),
                    DI_NORMAL,
                );
            }
            let text = RECT {
                left: r.left + px(6),
                top: cy + px(4),
                right: r.right - px(6),
                bottom: cy + px(24),
            };
            draw_text_in(
                hdc,
                strip.font,
                rgb(0xE6, 0xE6, 0xEA),
                &text,
                &card.title,
                DT_CENTER | DT_SINGLELINE | DT_VCENTER | DT_END_ELLIPSIS,
            );
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cards_keep_their_natural_size_while_they_fit() {
        let rects = card_rects(184, 1000, true, 3, 1.0);
        assert_eq!(rects.len(), 3);
        let r = rects[0];
        assert_eq!((r.right - r.left, r.bottom - r.top), (164, 123));
        let (before, after) = (rects[0].top, 1000 - rects[2].bottom);
        assert!((before - after).abs() <= 1, "centred along the axis");
    }

    #[test]
    fn cards_shrink_to_fit_and_overflow_is_dropped() {
        let rects = card_rects(184, 600, true, 6, 1.0);
        assert_eq!(rects.len(), 6);
        assert!(rects.iter().all(|r| r.bottom <= 590));
        let crowded = card_rects(184, 300, true, 20, 1.0);
        assert!(crowded.len() < 20);
        assert!(crowded
            .iter()
            .all(|r| r.bottom - r.top >= 48 && r.bottom <= 290));
    }

    #[test]
    fn horizontal_strips_lay_cards_out_left_to_right() {
        let rects = card_rects(1000, 134, false, 2, 1.0);
        assert_eq!(rects.len(), 2);
        assert!(rects[0].right <= rects[1].left);
        assert_eq!((rects[0].top, rects[0].bottom), (10, 124));
    }

    #[test]
    fn a_drop_lands_past_every_centre_it_crossed() {
        let rects = card_rects(184, 1000, true, 3, 1.0);
        let centre = |r: &RECT| (r.top + r.bottom) / 2;
        assert_eq!(drop_slot(0, centre(&rects[0]), &rects, true), 0);
        assert_eq!(drop_slot(0, centre(&rects[1]) + 1, &rects, true), 1);
        assert_eq!(drop_slot(0, 5000, &rects, true), 2);
        assert_eq!(drop_slot(2, -5000, &rects, true), 0);
    }
}
