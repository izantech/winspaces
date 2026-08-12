//! The theme combo's `WS_EX_NOACTIVATE` popup: geometry, its own wndproc,
//! painting, and committing a selection back into the owner window.

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
    CreateWindowExW, DefWindowProcW, DestroyWindow, GetSystemMetrics, PostMessageW, ShowWindow,
    SM_CYSCREEN, SW_SHOWNOACTIVATE, WM_ERASEBKGND, WM_LBUTTONUP, WM_MOUSEMOVE, WM_PAINT,
    WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_POPUP,
};
use winspaces_win32::dwm::{set_dark_mode, set_round_corners};
use winspaces_win32::gdi::color::Tint;
use winspaces_win32::gdi::draw::{draw_text_in, fill_round};
use winspaces_win32::gdi::surface::paint_surface;
use winspaces_win32::module::app_instance;
use winspaces_win32::text::encode_wide;

pub(crate) struct ComboPopup {
    pub(crate) hwnd: HWND,
    pub(crate) width: i32,
    pub(crate) item_h: i32,
    pub(crate) pad: i32,
    pub(crate) hover: Option<usize>,
}

fn combo_field_rect(win: &Win) -> Option<RECT> {
    win.layout
        .controls
        .iter()
        .find(|(id, _)| *id == ControlId::ComboTheme)
        .map(|(_, r)| RECT {
            left: r.left,
            top: r.top - win.scroll,
            right: r.right,
            bottom: r.bottom - win.scroll,
        })
}

pub(crate) unsafe fn open_combo(win: &mut Win) {
    close_combo(win);
    let Some(field) = combo_field_rect(win) else {
        return;
    };
    let px = px_of(win.scale);
    let item_h = px(36);
    let pad = px(4);
    let width = field.right - field.left;
    let height = item_h * ThemePref::ALL.len() as i32 + pad * 2;

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
        return;
    }
    set_dark_mode(hwnd, !win.pal.light);
    set_round_corners(hwnd);
    ShowWindow(hwnd, SW_SHOWNOACTIVATE);
    win.combo = Some(ComboPopup {
        hwnd,
        width,
        item_h,
        pad,
        hover: Some(win.state.theme_pref.index()),
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
                    let idx = ((y - combo.pad) / combo.item_h.max(1)) as usize;
                    let idx = if y < combo.pad || idx >= ThemePref::ALL.len() {
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
                        let idx = ((y - combo.pad) / combo.item_h.max(1)) as usize;
                        if y >= combo.pad && idx < ThemePref::ALL.len() {
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
    let h = combo.item_h * ThemePref::ALL.len() as i32 + combo.pad * 2;
    // Opaque popup surface (no backdrop behind a NOACTIVATE popup).
    let flyout = if pal.light { 0xF9 } else { 0x2C };
    let tint = Tint {
        r: flyout,
        g: flyout,
        b: flyout,
        alpha: 255,
    };
    paint_surface(hdc, combo.width, h, &tint, |mem| {
        for (i, pref) in ThemePref::ALL.iter().enumerate() {
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
            if win.state.theme_pref.index() == i {
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
                pref.label(),
                DT_SINGLELINE | DT_VCENTER | DT_LEFT | DT_END_ELLIPSIS,
            );
        }
    });
}

pub(crate) unsafe fn commit_combo(win: &mut Win, index: usize) {
    close_combo(win);
    let pref = ThemePref::ALL[index];
    if pref != win.state.theme_pref {
        win.state.theme_pref = pref;
        theme::save_pref(pref);
        win.pal = theme::build_palette(pref, win.mica);
        apply_frame_attributes(win.hwnd, &win.pal, win.mica);
    }
    relayout(win);
    InvalidateRect(win.hwnd, std::ptr::null(), 0);
}
