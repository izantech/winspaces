//! The settings window's own wndproc: keyboard nav, scrolling, mouse hover
//! and press/drag dispatch, DPI changes, and the timers.

use super::actions::{
    activate, after_action, handle_recorder_key, run_file_dialog, stop_recording,
};
use super::combo::{close_combo, commit_combo, hovered_or_selected, move_hover};
use super::layout::{
    clamp_scroll, ensure_focus_visible, focus_len, focused_control, hit_test, px_of, relayout,
    scrollbar_rects, viewport_h, HitTarget,
};
use super::pages::ControlId;
use super::render::on_paint;
use super::{
    apply_frame_attributes, controls, recorder, with_win, Win, TIMER_BANNER, TIMER_CAPTURE, WIN,
    WM_APP_COMBO_COMMIT, WM_APP_FILE_DIALOG, WM_DPICHANGED, WM_MOUSELEAVE,
};
use crate::theme;
use std::ptr::null_mut;
use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows_sys::Win32::Graphics::Gdi::InvalidateRect;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::VK_ESCAPE;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    GetKeyState, ReleaseCapture, SetCapture, TrackMouseEvent, TME_LEAVE, TRACKMOUSEEVENT, VK_DOWN,
    VK_END, VK_HOME, VK_NEXT, VK_PRIOR, VK_RETURN, VK_SHIFT, VK_SPACE, VK_TAB, VK_UP,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    DefWindowProcW, KillTimer, PostQuitMessage, SetWindowPos, SystemParametersInfoW, MINMAXINFO,
    SPI_GETWHEELSCROLLLINES, SWP_NOACTIVATE, SWP_NOZORDER, WA_INACTIVE, WM_ACTIVATE, WM_DESTROY,
    WM_ERASEBKGND, WM_GETMINMAXINFO, WM_KEYDOWN, WM_KILLFOCUS, WM_LBUTTONDOWN, WM_LBUTTONUP,
    WM_MOUSEMOVE, WM_MOUSEWHEEL, WM_NCLBUTTONDOWN, WM_PAINT, WM_SETTINGCHANGE, WM_SIZE,
    WM_SYSKEYDOWN, WM_TIMER,
};

unsafe fn wheel_scroll_lines() -> i32 {
    let mut lines: u32 = 3;
    SystemParametersInfoW(SPI_GETWHEELSCROLLLINES, 0, &mut lines as *mut _ as _, 0);
    lines.clamp(1, 20) as i32
}

unsafe fn scroll_by(win: &mut Win, delta: i32) {
    let before = win.scroll;
    win.scroll += delta;
    clamp_scroll(win);
    if win.scroll != before {
        if win.combo.is_some() {
            close_combo(win);
        }
        InvalidateRect(win.hwnd, std::ptr::null(), 0);
    }
}

unsafe fn on_key_down(win: &mut Win, vk: u32) {
    let px = px_of(win.scale);

    // Combo keyboard model while the dropdown is open.
    if win.combo.is_some() {
        match vk as u16 {
            VK_ESCAPE => close_combo(win),
            VK_UP | VK_DOWN => move_hover(win, vk as u16 == VK_DOWN),
            VK_RETURN | VK_SPACE => {
                if let Some(idx) = hovered_or_selected(win) {
                    commit_combo(win, idx);
                }
            }
            _ => {}
        }
        return;
    }

    // Recorder fallback when the LL hook could not install: plain focused
    // keydowns still record (global-hotkey combos excepted).
    if win.state.capturing.is_some() && !win.state.capture_hook {
        handle_recorder_key(win, vk);
        return;
    }
    if win.state.capturing.is_some() {
        // The LL hook owns the keyboard; swallow anything that leaks through.
        return;
    }

    match vk as u16 {
        VK_TAB => {
            win.keyboard_nav = true;
            let len = focus_len(win);
            if len == 0 {
                return;
            }
            let shift_down = GetKeyState(VK_SHIFT as i32) as u16 & 0x8000 != 0;
            win.focus = Some(match win.focus {
                None => {
                    if shift_down {
                        len - 1
                    } else {
                        0
                    }
                }
                Some(i) => {
                    if shift_down {
                        (i + len - 1) % len
                    } else {
                        (i + 1) % len
                    }
                }
            });
            ensure_focus_visible(win);
            InvalidateRect(win.hwnd, std::ptr::null(), 0);
        }
        VK_RETURN | VK_SPACE => {
            if win.keyboard_nav {
                if let Some(id) = focused_control(win) {
                    activate(win, id);
                }
            }
        }
        VK_UP => scroll_by(win, -px(40)),
        VK_DOWN => scroll_by(win, px(40)),
        VK_PRIOR => scroll_by(win, -viewport_h(win) + px(40)),
        VK_NEXT => scroll_by(win, viewport_h(win) - px(40)),
        VK_HOME => {
            win.scroll = 0;
            InvalidateRect(win.hwnd, std::ptr::null(), 0);
        }
        VK_END => {
            win.scroll = i32::MAX;
            clamp_scroll(win);
            InvalidateRect(win.hwnd, std::ptr::null(), 0);
        }
        _ => {}
    }
}

pub(crate) unsafe extern "system" fn settings_wnd_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match msg {
        WM_ERASEBKGND => 1,
        WM_PAINT => {
            on_paint(hwnd);
            0
        }
        WM_SIZE => {
            with_win(|win| {
                win.client_w = (lparam & 0xFFFF) as i32;
                win.client_h = ((lparam >> 16) & 0xFFFF) as i32;
                relayout(win);
            });
            0
        }
        WM_GETMINMAXINFO => {
            let scale = WIN.with(|s| {
                s.try_borrow()
                    .ok()
                    .and_then(|b| b.as_ref().map(|w| w.scale))
                    .unwrap_or(1.0)
            });
            let mmi = &mut *(lparam as *mut MINMAXINFO);
            mmi.ptMinTrackSize.x = (700.0 * scale) as i32;
            mmi.ptMinTrackSize.y = (500.0 * scale) as i32;
            0
        }
        WM_MOUSEMOVE => {
            let x = (lparam & 0xFFFF) as i16 as i32;
            let y = ((lparam >> 16) & 0xFFFF) as i16 as i32;
            with_win(|win| {
                if !win.tracking_leave {
                    let mut tme = TRACKMOUSEEVENT {
                        cbSize: std::mem::size_of::<TRACKMOUSEEVENT>() as u32,
                        dwFlags: TME_LEAVE,
                        hwndTrack: hwnd,
                        dwHoverTime: 0,
                    };
                    TrackMouseEvent(&mut tme);
                    win.tracking_leave = true;
                }
                if let Some(grab) = win.scrollbar_drag {
                    if let Some((track, thumb)) = scrollbar_rects(win) {
                        let track_h = track.bottom - track.top;
                        let thumb_h = thumb.bottom - thumb.top;
                        let denom = (track_h - thumb_h).max(1);
                        let max_scroll = (win.layout.content_h - viewport_h(win)).max(0);
                        let rel = (y - grab - track.top).clamp(0, denom);
                        win.scroll = ((rel as f32 / denom as f32) * max_scroll as f32) as i32;
                        clamp_scroll(win);
                        InvalidateRect(win.hwnd, std::ptr::null(), 0);
                    }
                    return;
                }
                let new_hover = match hit_test(win, x, y) {
                    HitTarget::Control(id) => Some(id),
                    _ => None,
                };
                if new_hover != win.hover {
                    win.hover = new_hover;
                    InvalidateRect(win.hwnd, std::ptr::null(), 0);
                }
            });
            0
        }
        WM_MOUSELEAVE => {
            with_win(|win| {
                win.tracking_leave = false;
                if win.hover.is_some() {
                    win.hover = None;
                    InvalidateRect(win.hwnd, std::ptr::null(), 0);
                }
            });
            0
        }
        WM_MOUSEWHEEL => {
            let delta = ((wparam >> 16) & 0xFFFF) as i16 as i32;
            with_win(|win| {
                let px = px_of(win.scale);
                let amount =
                    -(delta as f32 / 120.0 * (wheel_scroll_lines() * px(20)) as f32) as i32;
                scroll_by(win, amount);
            });
            0
        }
        WM_LBUTTONDOWN => {
            let x = (lparam & 0xFFFF) as i16 as i32;
            let y = ((lparam >> 16) & 0xFFFF) as i16 as i32;
            with_win(|win| {
                win.keyboard_nav = false;
                if win.combo.is_some() {
                    // Any main-window press light-dismisses the dropdown; a
                    // press on the combo field itself just closes (toggle).
                    close_combo(win);
                    InvalidateRect(win.hwnd, std::ptr::null(), 0);
                    if let HitTarget::Control(
                        ControlId::ComboTheme
                        | ControlId::ComboLanguage
                        | ControlId::ComboInnerGap
                        | ControlId::ComboOuterGap,
                    ) = hit_test(win, x, y)
                    {
                        return;
                    }
                }
                match hit_test(win, x, y) {
                    HitTarget::ScrollThumb => {
                        if let Some((_, thumb)) = scrollbar_rects(win) {
                            win.scrollbar_drag = Some(y - thumb.top);
                            SetCapture(hwnd);
                        }
                    }
                    HitTarget::ScrollTrack => {
                        if let Some((track, thumb)) = scrollbar_rects(win) {
                            let thumb_h = thumb.bottom - thumb.top;
                            let track_h = track.bottom - track.top;
                            let denom = (track_h - thumb_h).max(1);
                            let max_scroll = (win.layout.content_h - viewport_h(win)).max(0);
                            let rel = (y - track.top - thumb_h / 2).clamp(0, denom);
                            win.scroll = ((rel as f32 / denom as f32) * max_scroll as f32) as i32;
                            clamp_scroll(win);
                            InvalidateRect(win.hwnd, std::ptr::null(), 0);
                        }
                    }
                    HitTarget::Control(id) => {
                        win.pressed = Some(id);
                        SetCapture(hwnd);
                        InvalidateRect(win.hwnd, std::ptr::null(), 0);
                    }
                    HitTarget::None => {}
                }
            });
            0
        }
        WM_LBUTTONUP => {
            let x = (lparam & 0xFFFF) as i16 as i32;
            let y = ((lparam >> 16) & 0xFFFF) as i16 as i32;
            ReleaseCapture();
            with_win(|win| {
                if win.scrollbar_drag.take().is_some() {
                    InvalidateRect(win.hwnd, std::ptr::null(), 0);
                    return;
                }
                let pressed = win.pressed.take();
                if let (Some(p), HitTarget::Control(h)) = (pressed, hit_test(win, x, y)) {
                    if p == h {
                        activate(win, p);
                    }
                }
                InvalidateRect(win.hwnd, std::ptr::null(), 0);
            });
            0
        }
        WM_NCLBUTTONDOWN => {
            with_win(|win| {
                if win.combo.is_some() {
                    close_combo(win);
                }
            });
            DefWindowProcW(hwnd, msg, wparam, lparam)
        }
        WM_KEYDOWN => {
            with_win(|win| on_key_down(win, wparam as u32));
            0
        }
        WM_SYSKEYDOWN => {
            let mut handled = false;
            with_win(|win| {
                if win.state.capturing.is_some() && !win.state.capture_hook {
                    handle_recorder_key(win, wparam as u32);
                    handled = true;
                }
            });
            if handled {
                0
            } else {
                DefWindowProcW(hwnd, msg, wparam, lparam)
            }
        }
        m if m == recorder::WM_APP_RECORDER_KEY => {
            with_win(|win| handle_recorder_key(win, wparam as u32));
            0
        }
        WM_APP_COMBO_COMMIT => {
            with_win(|win| commit_combo(win, wparam));
            0
        }

        // Deliberately outside `with_win`: the dialog owns the message loop
        // until the user answers it, and this wndproc keeps running under it.
        WM_APP_FILE_DIALOG => {
            run_file_dialog(hwnd, wparam);
            0
        }
        WM_TIMER => {
            match wparam {
                TIMER_BANNER => {
                    KillTimer(hwnd, TIMER_BANNER);
                    with_win(|win| {
                        if win.state.banner.take().is_some() {
                            relayout(win);
                            InvalidateRect(win.hwnd, std::ptr::null(), 0);
                        }
                    });
                }
                TIMER_CAPTURE => {
                    KillTimer(hwnd, TIMER_CAPTURE);
                    with_win(|win| {
                        win.state.finish_capture_reload();
                        after_action(win);
                    });
                }
                _ => {}
            }
            0
        }
        WM_ACTIVATE => {
            if (wparam & 0xFFFF) as u32 == WA_INACTIVE {
                with_win(|win| {
                    stop_recording(win);
                    // Activation moving to our own dropdown is not a dismiss:
                    // closing here would destroy the popup mid-click.
                    let to_popup = win.combo.as_ref().is_some_and(|c| c.hwnd == lparam as HWND);
                    if win.combo.is_some() && !to_popup {
                        close_combo(win);
                    }
                });
            }
            0
        }
        WM_KILLFOCUS => {
            with_win(|win| stop_recording(win));
            0
        }
        WM_SETTINGCHANGE => {
            // Theme, accent, or high-contrast change: rebuild the palette
            // (cheap) and re-apply the caption/backdrop attributes.
            with_win(|win| {
                win.pal = theme::build_palette(win.state.theme_pref, win.mica);
                apply_frame_attributes(win.hwnd, &win.pal, win.mica);
                InvalidateRect(win.hwnd, std::ptr::null(), 0);
            });
            DefWindowProcW(hwnd, msg, wparam, lparam)
        }
        m if m == WM_DPICHANGED => {
            let new_dpi = (wparam & 0xFFFF) as u32;
            let suggested = &*(lparam as *const RECT);
            let (x, y, w, h) = (
                suggested.left,
                suggested.top,
                suggested.right - suggested.left,
                suggested.bottom - suggested.top,
            );
            SetWindowPos(hwnd, null_mut(), x, y, w, h, SWP_NOZORDER | SWP_NOACTIVATE);
            with_win(|win| {
                win.scale = if new_dpi > 0 {
                    new_dpi as f32 / 96.0
                } else {
                    1.0
                };
                controls::delete_fonts(&win.fonts);
                win.fonts = controls::create_fonts(win.scale);
                relayout(win);
                InvalidateRect(win.hwnd, std::ptr::null(), 0);
            });
            0
        }
        WM_DESTROY => {
            recorder::stop_capture();
            with_win(|win| {
                if win.combo.is_some() {
                    close_combo(win);
                }
            });
            KillTimer(hwnd, TIMER_BANNER);
            KillTimer(hwnd, TIMER_CAPTURE);
            PostQuitMessage(0);
            0
        }
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}
