//! The combo popup for theme, language and gap presets: geometry, its own
//! wndproc, painting, and committing a selection back into the owner window.

use super::layout::{px_of, relayout};
use super::pages::ControlId;
use super::{apply_frame_attributes, with_win, Win, POPUP_CLASS, WM_APP_COMBO_COMMIT};
use crate::theme::{self, ThemePref};
use std::ptr::null_mut;
use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows_sys::Win32::Graphics::Gdi::{
    BeginPaint, ClientToScreen, EndPaint, InvalidateRect, DT_END_ELLIPSIS, DT_LEFT, DT_SINGLELINE,
    DT_VCENTER, HDC, PAINTSTRUCT,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, GetSystemMetrics, PostMessageW, SetWindowTextW,
    ShowWindow, MA_NOACTIVATE, SM_CYSCREEN, SW_SHOWNOACTIVATE, WM_ERASEBKGND, WM_LBUTTONUP,
    WM_MOUSEACTIVATE, WM_MOUSEMOVE, WM_PAINT, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TOPMOST,
    WS_POPUP,
};
use winspaces_common::i18n::{self, t, SYSTEM_TAG};
use winspaces_common::{log_info, tr, Lang, Msg};
use winspaces_win32::dwm::{set_dark_mode, set_round_corners};
use winspaces_win32::gdi::color::Tint;
use winspaces_win32::gdi::draw::{draw_text_in, fill_round};
use winspaces_win32::gdi::surface::paint_surface;
use winspaces_win32::module::app_instance;
use winspaces_win32::text::encode_wide;

pub const GAP_PRESETS: &[u32] = &[0, 4, 8, 12, 16, 24];

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum ComboKind {
    Theme,
    Language,
    InnerGap,
    OuterGap,
}

pub(crate) struct ComboPopup {
    pub(crate) hwnd: HWND,
    pub(crate) kind: ComboKind,
    pub(crate) width: i32,
    pub(crate) item_h: i32,
    pub(crate) pad: i32,
    pub(crate) hover: Option<usize>,
}

fn combo_field_rect(win: &Win, kind: ComboKind) -> Option<RECT> {
    let target_id = match kind {
        ComboKind::Theme => ControlId::ComboTheme,
        ComboKind::Language => ControlId::ComboLanguage,
        ComboKind::InnerGap => ControlId::ComboInnerGap,
        ComboKind::OuterGap => ControlId::ComboOuterGap,
    };
    win.layout
        .controls
        .iter()
        .find(|(id, _)| *id == target_id)
        .map(|(_, r)| RECT {
            left: r.left,
            top: r.top - win.scroll,
            right: r.right,
            bottom: r.bottom - win.scroll,
        })
}

fn combo_item_count(kind: ComboKind) -> usize {
    match kind {
        ComboKind::Theme => ThemePref::ALL.len(),
        // Row 0 is "follow Windows"; the rest are `Lang::ALL` in order.
        ComboKind::Language => 1 + Lang::ALL.len(),
        ComboKind::InnerGap | ComboKind::OuterGap => GAP_PRESETS.len(),
    }
}

/// Row index of the current `Config.language` in the language combo.
fn language_index(setting: &str) -> usize {
    if setting == SYSTEM_TAG {
        return 0;
    }
    Lang::from_tag(setting)
        .and_then(|lang| Lang::ALL.iter().position(|l| *l == lang))
        .map(|i| i + 1)
        .unwrap_or(0)
}

/// Combo label for the current `Config.language`.
pub(crate) fn language_label(setting: &str) -> &'static str {
    match Lang::from_tag(setting) {
        Some(lang) if setting != SYSTEM_TAG => lang.label(),
        _ => t(Msg::LangSystem),
    }
}

fn combo_selected_index(win: &Win, kind: ComboKind) -> usize {
    match kind {
        ComboKind::Theme => win.state.theme_pref.index(),
        ComboKind::Language => language_index(&win.state.config.language),
        ComboKind::InnerGap => GAP_PRESETS
            .iter()
            .position(|&g| g == win.state.config.tiling.inner_gap)
            .unwrap_or(2),
        ComboKind::OuterGap => GAP_PRESETS
            .iter()
            .position(|&g| g == win.state.config.tiling.outer_gap)
            .unwrap_or(2),
    }
}

/// Move the highlight in the open dropdown, wrapping within *its* own item
/// count (the theme, language and gap combos do not have the same length).
pub(crate) unsafe fn move_hover(win: &mut Win, down: bool) {
    let Some(kind) = win.combo.as_ref().map(|c| c.kind) else {
        return;
    };
    let len = combo_item_count(kind);
    if len == 0 {
        return;
    }
    let cur = win
        .combo
        .as_ref()
        .and_then(|c| c.hover)
        .unwrap_or_else(|| combo_selected_index(win, kind));
    let next = if down {
        (cur + 1) % len
    } else {
        (cur + len - 1) % len
    };
    if let Some(combo) = win.combo.as_mut() {
        combo.hover = Some(next);
        InvalidateRect(combo.hwnd, std::ptr::null(), 0);
    }
}

/// The row Return/Space commits: the highlight, else the current value.
pub(crate) fn hovered_or_selected(win: &Win) -> Option<usize> {
    let kind = win.combo.as_ref().map(|c| c.kind)?;
    Some(
        win.combo
            .as_ref()
            .and_then(|c| c.hover)
            .unwrap_or_else(|| combo_selected_index(win, kind)),
    )
}

pub(crate) unsafe fn open_combo(win: &mut Win, kind: ComboKind) {
    close_combo(win);
    let Some(field) = combo_field_rect(win, kind) else {
        log_info!("Settings: {:?} dropdown has no laid-out field", kind);
        return;
    };
    let px = px_of(win.scale);
    let item_h = px(36);
    let pad = px(4);
    let width = field.right - field.left;
    let count = combo_item_count(kind);
    let height = item_h * count as i32 + pad * 2;

    let mut origin = POINT {
        x: field.left,
        y: field.bottom + px(4),
    };
    ClientToScreen(win.hwnd, &mut origin);
    // Flip above the field when the popup would leave the screen.
    let sh = GetSystemMetrics(SM_CYSCREEN);
    if origin.y + height > sh {
        origin.y -= height + (field.bottom - field.top) + px(8);
    }

    let class_name = encode_wide(POPUP_CLASS);
    let hwnd = CreateWindowExW(
        WS_EX_TOOLWINDOW | WS_EX_TOPMOST | WS_EX_NOACTIVATE,
        class_name.as_ptr(),
        std::ptr::null(),
        WS_POPUP,
        origin.x,
        origin.y,
        width,
        height,
        win.hwnd,
        null_mut(),
        app_instance(),
        null_mut(),
    );
    if hwnd.is_null() {
        log_info!("Settings: {:?} dropdown window creation failed", kind);
        return;
    }
    log_info!(
        "Settings: {:?} dropdown opened at {},{} ({}x{}, {} items)",
        kind,
        origin.x,
        origin.y,
        width,
        height,
        count
    );
    set_dark_mode(hwnd, !win.pal.light);
    set_round_corners(hwnd);
    ShowWindow(hwnd, SW_SHOWNOACTIVATE);
    win.combo = Some(ComboPopup {
        hwnd,
        kind,
        width,
        item_h,
        pad,
        hover: Some(combo_selected_index(win, kind)),
    });
}

pub(crate) unsafe fn close_combo(win: &mut Win) {
    if let Some(combo) = win.combo.take() {
        DestroyWindow(combo.hwnd);
    }
}

pub(crate) unsafe extern "system" fn popup_wnd_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match msg {
        WM_ERASEBKGND => 1,
        // Without this, DefWindowProc answers MA_ACTIVATE and the click moves
        // activation off the settings window, whose WM_ACTIVATE light-dismiss
        // destroys this popup before it ever sees the button-up. The dropdown
        // then just closes and the row is never committed.
        WM_MOUSEACTIVATE => MA_NOACTIVATE as LRESULT,
        WM_PAINT => {
            let mut ps: PAINTSTRUCT = std::mem::zeroed();
            let hdc = BeginPaint(hwnd, &mut ps);
            super::WIN.with(|s| {
                if let Ok(borrow) = s.try_borrow() {
                    if let Some(win) = borrow.as_ref() {
                        if let Some(combo) = &win.combo {
                            if combo.hwnd == hwnd {
                                draw_combo(hdc, win, combo);
                            }
                        }
                    }
                }
            });
            EndPaint(hwnd, &ps);
            0
        }
        WM_MOUSEMOVE => {
            let y = (lparam >> 16) as i16 as i32;
            with_win(|win| {
                if let Some(combo) = win.combo.as_mut() {
                    let count = combo_item_count(combo.kind);
                    let idx = ((y - combo.pad) / combo.item_h.max(1)) as usize;
                    let idx = if y < combo.pad || idx >= count {
                        None
                    } else {
                        Some(idx)
                    };
                    if idx != combo.hover {
                        combo.hover = idx;
                        InvalidateRect(combo.hwnd, std::ptr::null(), 0);
                    }
                }
            });
            0
        }
        WM_LBUTTONUP => {
            let y = (lparam >> 16) as i16 as i32;
            let mut commit: Option<usize> = None;
            let mut owner: HWND = null_mut();
            with_win(|win| {
                if let Some(combo) = &win.combo {
                    if combo.hwnd == hwnd {
                        let count = combo_item_count(combo.kind);
                        let idx = ((y - combo.pad) / combo.item_h.max(1)) as usize;
                        if y >= combo.pad && idx < count {
                            commit = Some(idx);
                            owner = win.hwnd;
                        }
                    }
                }
            });
            if let Some(idx) = commit {
                PostMessageW(owner, WM_APP_COMBO_COMMIT, idx, 0);
            }
            0
        }
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

unsafe fn draw_combo(hdc: HDC, win: &Win, combo: &ComboPopup) {
    let px = px_of(win.scale);
    let pal = &win.pal;
    let count = combo_item_count(combo.kind);
    let selected = combo_selected_index(win, combo.kind);
    let h = combo.item_h * count as i32 + combo.pad * 2;
    // Opaque popup surface (no backdrop behind a NOACTIVATE popup).
    let flyout = if pal.light { 0xF9 } else { 0x2C };
    let tint = Tint {
        r: flyout,
        g: flyout,
        b: flyout,
        alpha: 255,
    };

    let labels: Vec<String> = match combo.kind {
        ComboKind::Theme => ThemePref::ALL
            .iter()
            .map(|p| p.label().to_string())
            .collect(),
        ComboKind::Language => std::iter::once(t(Msg::LangSystem).to_string())
            .chain(Lang::ALL.iter().map(|l| l.label().to_string()))
            .collect(),
        ComboKind::InnerGap | ComboKind::OuterGap => GAP_PRESETS
            .iter()
            .map(|&g| tr!(Msg::SettingsUnitPx, n = g))
            .collect(),
    };

    paint_surface(hdc, combo.width, h, &tint, |mem| {
        for (i, label) in labels.iter().enumerate() {
            let top = combo.pad + i as i32 * combo.item_h;
            let r = RECT {
                left: px(4),
                top,
                right: combo.width - px(4),
                bottom: top + combo.item_h,
            };
            if combo.hover == Some(i) {
                fill_round(mem, &r, px(4), pal.ctl_hover, pal.ctl_hover);
            }
            if selected == i {
                let pill = RECT {
                    left: px(4),
                    top: top + (combo.item_h - px(16)) / 2,
                    right: px(4) + px(3),
                    bottom: top + (combo.item_h + px(16)) / 2,
                };
                fill_round(mem, &pill, px(3), pal.accent, pal.accent);
            }
            let text_rect = RECT {
                left: px(16),
                top,
                right: combo.width - px(8),
                bottom: top + combo.item_h,
            };
            draw_text_in(
                mem,
                win.fonts.body,
                pal.text,
                &text_rect,
                label,
                DT_SINGLELINE | DT_VCENTER | DT_LEFT | DT_END_ELLIPSIS,
            );
        }
    });
}

pub(crate) unsafe fn commit_combo(win: &mut Win, index: usize) {
    let Some(combo) = win.combo.take() else {
        return;
    };
    DestroyWindow(combo.hwnd);
    log_info!(
        "Settings: {:?} dropdown committed row {}",
        combo.kind,
        index
    );

    match combo.kind {
        ComboKind::Theme => {
            if index < ThemePref::ALL.len() {
                let pref = ThemePref::ALL[index];
                if pref != win.state.theme_pref {
                    win.state.theme_pref = pref;
                    theme::save_pref(pref);
                    win.pal = theme::build_palette(pref, win.mica);
                    apply_frame_attributes(win.hwnd, &win.pal, win.mica);
                }
            }
        }
        ComboKind::Language => {
            let new_tag = if index == 0 {
                SYSTEM_TAG.to_string()
            } else {
                match Lang::ALL.get(index - 1) {
                    Some(lang) => lang.tag().to_string(),
                    None => return,
                }
            };
            if new_tag != win.state.config.language {
                win.state.config.language = new_tag;
                // Switch before the autosave so its banner is already in
                // the new language; the relayout below rebuilds every
                // other string.
                i18n::set_current(Lang::resolve(&win.state.config.language));
                let title = encode_wide(t(Msg::SettingsTitle));
                SetWindowTextW(win.hwnd, title.as_ptr());
                win.state.autosave(t(Msg::ReasonLanguage));
            }
        }
        ComboKind::InnerGap => {
            if index < GAP_PRESETS.len() {
                let gap = GAP_PRESETS[index];
                if win.state.config.tiling.inner_gap != gap {
                    win.state.config.tiling.inner_gap = gap;
                    win.state.autosave(t(Msg::ReasonInnerGap));
                }
            }
        }
        ComboKind::OuterGap => {
            if index < GAP_PRESETS.len() {
                let gap = GAP_PRESETS[index];
                if win.state.config.tiling.outer_gap != gap {
                    win.state.config.tiling.outer_gap = gap;
                    win.state.autosave(t(Msg::ReasonOuterGap));
                }
            }
        }
    }

    relayout(win);
    InvalidateRect(win.hwnd, std::ptr::null(), 0);
}
