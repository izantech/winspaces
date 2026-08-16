//! Relayout (page/banner/content-driven), focus order, scrollbar geometry,
//! and hit-testing against the laid-out `Win`.

use super::pages::{self, ControlId, LayoutParams, Page};
use super::Win;
use super::{NAV_ITEM_GAP, NAV_ITEM_H, NAV_W};
use windows_sys::Win32::Foundation::RECT;
use windows_sys::Win32::Graphics::Gdi::{CreateCompatibleDC, DeleteDC, GetDC, ReleaseDC};
use winspaces_win32::dpi;
use winspaces_win32::gdi::draw::measure_text;

pub(crate) fn px_of(scale: f32) -> impl Fn(i32) -> i32 {
    move |v: i32| dpi::px(scale, v)
}

pub(crate) fn relayout(win: &mut Win) {
    let px = px_of(win.scale);

    for (i, _page) in Page::ALL.iter().enumerate() {
        win.nav_rects[i] = RECT {
            left: px(12),
            top: px(12) + i as i32 * (px(NAV_ITEM_H) + px(NAV_ITEM_GAP)),
            right: px(NAV_W) - px(12),
            bottom: px(12) + i as i32 * (px(NAV_ITEM_H) + px(NAV_ITEM_GAP)) + px(NAV_ITEM_H),
        };
    }

    let viewport_x = px(NAV_W);
    let viewport_w = (win.client_w - viewport_x).max(px(240));
    let content_w = (viewport_w - px(64)).min(px(1000)).max(px(200));
    let origin_x = viewport_x + ((viewport_w - content_w) / 2).max(px(32));

    let theme_label = win.state.theme_pref.label();
    let params = LayoutParams {
        origin_x,
        width: content_w,
        scale: win.scale,
        banner_open: win.state.banner.is_some(),
        page: win.state.page,
        config: &win.state.config,
        machine_name: &win.state.machine_name,
        daemon_running: win.state.daemon_running,
        theme_label,
    };

    // Text measurement against a scratch DC with the real fonts.
    unsafe {
        let screen = GetDC(std::ptr::null_mut());
        let hdc = CreateCompatibleDC(screen);
        let body = win.fonts.body;
        let caption = win.fonts.caption;
        win.layout = pages::layout(
            &params,
            |s| measure_text(hdc, body, s),
            |s| measure_text(hdc, caption, s),
        );
        DeleteDC(hdc);
        ReleaseDC(std::ptr::null_mut(), screen);
    }

    clamp_scroll(win);
}

pub(crate) fn viewport_h(win: &Win) -> i32 {
    win.client_h
}

pub(crate) fn clamp_scroll(win: &mut Win) {
    let max = (win.layout.content_h - viewport_h(win)).max(0);
    win.scroll = win.scroll.clamp(0, max);
}

/// Focus order: the nav items, then the content controls.
pub(crate) fn focus_len(win: &Win) -> usize {
    Page::ALL.len() + win.layout.controls.len()
}

pub(crate) fn focused_control(win: &Win) -> Option<ControlId> {
    match win.focus {
        Some(i) if i < Page::ALL.len() => Some(ControlId::Nav(Page::ALL[i])),
        Some(i) => win
            .layout
            .controls
            .get(i - Page::ALL.len())
            .map(|(id, _)| *id),
        None => None,
    }
}

pub(crate) fn ensure_focus_visible(win: &mut Win) {
    let px = px_of(win.scale);
    if let Some(i) = win.focus {
        if i >= Page::ALL.len() {
            if let Some((_, rect)) = win.layout.controls.get(i - Page::ALL.len()) {
                let top = rect.top - win.scroll;
                let bottom = rect.bottom - win.scroll;
                if top < px(8) {
                    win.scroll += top - px(16);
                } else if bottom > win.client_h - px(8) {
                    win.scroll += bottom - win.client_h + px(16);
                }
                clamp_scroll(win);
            }
        }
    }
}

#[derive(Clone, Copy, PartialEq)]
pub(crate) enum HitTarget {
    Control(ControlId),
    ScrollThumb,
    ScrollTrack,
    None,
}

fn pt_in(r: &RECT, x: i32, y: i32) -> bool {
    x >= r.left && x < r.right && y >= r.top && y < r.bottom
}

/// Scrollbar geometry: (track, thumb) in window coordinates, when scrollable.
pub(crate) fn scrollbar_rects(win: &Win) -> Option<(RECT, RECT)> {
    let px = px_of(win.scale);
    let vh = viewport_h(win);
    if win.layout.content_h <= vh {
        return None;
    }
    let track = RECT {
        left: win.client_w - px(12),
        top: px(4),
        right: win.client_w - px(4),
        bottom: win.client_h - px(4),
    };
    let track_h = track.bottom - track.top;
    let thumb_h = ((vh as f32 / win.layout.content_h as f32) * track_h as f32) as i32;
    let thumb_h = thumb_h.max(px(24));
    let max_scroll = (win.layout.content_h - vh).max(1);
    let thumb_top =
        track.top + ((win.scroll as f32 / max_scroll as f32) * (track_h - thumb_h) as f32) as i32;
    let thumb = RECT {
        left: track.left,
        top: thumb_top,
        right: track.right,
        bottom: thumb_top + thumb_h,
    };
    Some((track, thumb))
}

pub(crate) fn hit_test(win: &Win, x: i32, y: i32) -> HitTarget {
    if let Some((track, thumb)) = scrollbar_rects(win) {
        if pt_in(&thumb, x, y) {
            return HitTarget::ScrollThumb;
        }
        if pt_in(&track, x, y) {
            return HitTarget::ScrollTrack;
        }
    }
    for (i, r) in win.nav_rects.iter().enumerate() {
        if pt_in(r, x, y) {
            return HitTarget::Control(ControlId::Nav(Page::ALL[i]));
        }
    }
    let px = px_of(win.scale);
    if x >= px(NAV_W) {
        let cy = y + win.scroll;
        // Trailing controls first (they sit on top of their card), then
        // whole-card click targets.
        for (id, r) in &win.layout.controls {
            if !matches!(id, ControlId::NavCard(..)) && pt_in(r, x, cy) {
                return HitTarget::Control(*id);
            }
        }
        for (id, r) in &win.layout.controls {
            if matches!(id, ControlId::NavCard(..)) && pt_in(r, x, cy) {
                return HitTarget::Control(*id);
            }
        }
    }
    HitTarget::None
}
