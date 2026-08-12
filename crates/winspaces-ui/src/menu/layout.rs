//! Sizing (font metrics -> row/window dimensions) and hit-testing. Pure
//! geometry over the `MenuWindow`/`MenuState` the parent module owns.

use super::{Fonts, MenuEntry, MenuState, MenuWindow, Row};
use super::{
    CHEVRON_W, HEADER_H, ICON_X, LABEL_GAP, MAX_W, MIN_W, PAD_V, RIGHT_PAD, ROW_H, SEP_H, TEXT_X,
};
use windows_sys::Win32::Foundation::POINT;
use windows_sys::Win32::Graphics::Gdi::{CreateCompatibleDC, DeleteDC, GetDC, ReleaseDC};
use winspaces_win32::dpi::px;
use winspaces_win32::gdi::draw::measure_text;

pub(crate) unsafe fn layout_window(
    entries: &[MenuEntry],
    fonts: &Fonts,
    scale: f32,
) -> (Vec<Row>, i32, i32) {
    let px = |v: i32| px(scale, v);
    let hdc_screen = GetDC(std::ptr::null_mut());
    let hdc = CreateCompatibleDC(hdc_screen);

    let any_chevron = entries
        .iter()
        .any(|e| matches!(e, MenuEntry::Item(it) if it.submenu.is_some()));

    let mut rows = Vec::with_capacity(entries.len());
    let mut y = px(PAD_V);
    let mut max_w = px(MIN_W);

    for entry in entries {
        let height = match entry {
            MenuEntry::Header(_) => px(HEADER_H),
            MenuEntry::Separator => px(SEP_H),
            MenuEntry::Item(_) => px(ROW_H),
        };
        rows.push(Row { top: y, height });
        y += height;

        let req = match entry {
            MenuEntry::Header(text) => px(ICON_X) + measure_text(hdc, fonts.small, text) + px(16),
            MenuEntry::Separator => 0,
            MenuEntry::Item(it) => {
                let mut w = px(TEXT_X) + measure_text(hdc, fonts.text, &it.label) + px(LABEL_GAP);
                if let Some(s) = &it.shortcut {
                    w += measure_text(hdc, fonts.small, s);
                }
                if any_chevron {
                    w += px(CHEVRON_W);
                }
                w + px(RIGHT_PAD)
            }
        };
        max_w = max_w.max(req);
    }

    DeleteDC(hdc);
    ReleaseDC(std::ptr::null_mut(), hdc_screen);

    (rows, max_w.min(px(MAX_W)), y + px(PAD_V))
}

#[derive(PartialEq, Clone, Copy)]
pub(crate) enum Hit {
    Root(usize),
    RootBlank,
    Sub(usize),
    SubBlank,
    Outside,
}

pub(crate) fn hit_test_window(win: &MenuWindow, pt: POINT) -> Option<Option<usize>> {
    if pt.x < win.x || pt.x >= win.x + win.width || pt.y < win.y || pt.y >= win.y + win.height {
        return None;
    }
    let local_y = pt.y - win.y;
    for (idx, row) in win.rows.iter().enumerate() {
        if local_y >= row.top && local_y < row.top + row.height {
            if win.entries[idx].is_selectable() {
                return Some(Some(idx));
            }
            return Some(None);
        }
    }
    Some(None)
}

pub(crate) fn hit_test(state: &MenuState, pt: POINT) -> Hit {
    if let Some(sub) = &state.sub {
        if let Some(row) = hit_test_window(sub, pt) {
            return match row {
                Some(idx) => Hit::Sub(idx),
                None => Hit::SubBlank,
            };
        }
    }
    if let Some(row) = hit_test_window(&state.root, pt) {
        return match row {
            Some(idx) => Hit::Root(idx),
            None => Hit::RootBlank,
        };
    }
    Hit::Outside
}

pub(crate) fn step_selection(
    entries: &[MenuEntry],
    current: Option<usize>,
    dir: i32,
) -> Option<usize> {
    let len = entries.len() as i32;
    if len == 0 {
        return None;
    }
    let mut idx = match current {
        Some(c) => c as i32,
        None if dir > 0 => -1,
        None => len,
    };
    for _ in 0..len {
        idx = (idx + dir).rem_euclid(len);
        if entries[idx as usize].is_selectable() {
            return Some(idx as usize);
        }
    }
    None
}
