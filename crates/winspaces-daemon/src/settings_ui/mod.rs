//! Native settings window (`winspaces.exe --settings`). Runs in its own
//! process instance of the daemon binary — `run_settings` is only reachable
//! from the `--settings` CLI branch, so the daemon itself never touches any
//! of this state.
//!
//! Rendering is the menu.rs technique writ large: one `WS_OVERLAPPEDWINDOW`
//! with a real Mica backdrop (`DWMWA_SYSTEMBACKDROP_TYPE`), painted per frame
//! into a premultiplied 32bpp DIB with a semi-transparent theme tint so the
//! backdrop shows through. Every control is an owner-drawn region of this one
//! window; the only child surface is the theme combo's `WS_EX_NOACTIVATE`
//! dropdown popup. See docs/settings-ui.md.

mod autostart;
mod controls;
mod pages;
mod recorder;
mod state;
/// Theme resolution is shared with the tray menu (`menu.rs`), which resolves
/// the same preference on every open so both surfaces always agree.
pub mod theme;

use controls::{Fonts, Vis};
use pages::{ControlId, LaidItem, LaidTrailing, Layout, LayoutParams, Page};
use state::SettingsState;
use std::cell::RefCell;
use std::ptr::null_mut;
use theme::{Palette, ThemePref};

use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows_sys::Win32::Graphics::Dwm::{
    DwmExtendFrameIntoClientArea, DwmSetWindowAttribute, DWMWA_SYSTEMBACKDROP_TYPE,
    DWMWA_USE_IMMERSIVE_DARK_MODE,
};
use windows_sys::Win32::Graphics::Gdi::{
    BeginPaint, ClientToScreen, CreateCompatibleDC, DeleteDC, EndPaint, GetDC, InvalidateRect,
    ReleaseDC, DT_END_ELLIPSIS, DT_LEFT, DT_SINGLELINE, DT_VCENTER, PAINTSTRUCT,
};
use windows_sys::Win32::UI::Controls::MARGINS;
use windows_sys::Win32::UI::HiDpi::GetDpiForWindow;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    GetKeyState, ReleaseCapture, SetCapture, TrackMouseEvent, TME_LEAVE, TRACKMOUSEEVENT, VK_DOWN,
    VK_END, VK_ESCAPE, VK_HOME, VK_NEXT, VK_PRIOR, VK_RETURN, VK_SHIFT, VK_SPACE, VK_TAB, VK_UP,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, FindWindowW, GetClientRect, GetSystemMetrics,
    KillTimer, LoadCursorW, PostMessageW, PostQuitMessage, RegisterClassExW, SetForegroundWindow,
    SetTimer, SetWindowPos, ShowWindow, SystemParametersInfoW, TranslateMessage, CS_HREDRAW,
    CS_VREDRAW, CW_USEDEFAULT, IDC_ARROW, MINMAXINFO, MSG, SM_CXSCREEN, SM_CYSCREEN,
    SPI_GETWHEELSCROLLLINES, SWP_NOACTIVATE, SWP_NOZORDER, SW_RESTORE, SW_SHOW, WM_ACTIVATE,
    WM_APP, WM_DESTROY, WM_ERASEBKGND, WM_GETMINMAXINFO, WM_KEYDOWN, WM_KILLFOCUS, WM_LBUTTONDOWN,
    WM_LBUTTONUP, WM_MOUSEMOVE, WM_MOUSEWHEEL, WM_NCLBUTTONDOWN, WM_PAINT, WM_SETTINGCHANGE,
    WM_SIZE, WM_SYSKEYDOWN, WM_TIMER, WNDCLASSEXW, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW,
    WS_EX_TOPMOST, WS_OVERLAPPEDWINDOW, WS_POPUP,
};

const WM_MOUSELEAVE: u32 = 0x02A3;

use crate::log_info;
use crate::menu::win_build;
use crate::tray::encode_wide;

const SETTINGS_CLASS: &str = "WinSpacesSettingsClass";
const SETTINGS_TITLE: &str = "WinSpaces Settings";
const POPUP_CLASS: &str = "WinSpacesSettingsPopup";

/// Combo popup commits a selection: wparam = ThemePref index.
const WM_APP_COMBO_COMMIT: u32 = WM_APP + 71;

const WM_DPICHANGED: u32 = 0x02E0;

const TIMER_BANNER: usize = 1;
const TIMER_CAPTURE: usize = 2;

/// Nav rail metrics (96-dpi dips).
const NAV_W: i32 = 240;
const NAV_ITEM_H: i32 = 40;
const NAV_ITEM_GAP: i32 = 4;

struct ComboPopup {
    hwnd: HWND,
    width: i32,
    item_h: i32,
    pad: i32,
    hover: Option<usize>,
}

struct Win {
    hwnd: HWND,
    scale: f32,
    fonts: Fonts,
    pal: Palette,
    mica: bool,
    state: SettingsState,
    layout: Layout,
    nav_rects: [RECT; 3],
    scroll: i32,
    hover: Option<ControlId>,
    pressed: Option<ControlId>,
    /// Index into the focus order (nav items then content controls).
    focus: Option<usize>,
    /// Focus rings only show for keyboard-driven focus (Win11 behavior).
    keyboard_nav: bool,
    combo: Option<ComboPopup>,
    tracking_leave: bool,
    /// Offset between the grab point and the thumb top while dragging.
    scrollbar_drag: Option<i32>,
    client_w: i32,
    client_h: i32,
}

thread_local! {
    static WIN: RefCell<Option<Win>> = const { RefCell::new(None) };
}

fn with_win<F: FnOnce(&mut Win)>(f: F) {
    WIN.with(|s| {
        if let Ok(mut borrow) = s.try_borrow_mut() {
            if let Some(win) = borrow.as_mut() {
                f(win);
            }
        }
    });
}

/// Entry point for `winspaces.exe --settings`. Blocks until the window closes.
pub fn run_settings() {
    unsafe {
        // Single settings instance: focus the existing window instead.
        let class_name = encode_wide(SETTINGS_CLASS);
        let title = encode_wide(SETTINGS_TITLE);
        let existing = FindWindowW(class_name.as_ptr(), title.as_ptr());
        if !existing.is_null() {
            ShowWindow(existing, SW_RESTORE);
            SetForegroundWindow(existing);
            return;
        }

        let state = SettingsState::new();
        let mica = win_build() >= 22621;
        let pal = theme::build_palette(state.theme_pref, mica);

        register_class(SETTINGS_CLASS, Some(settings_wnd_proc));
        register_class(POPUP_CLASS, Some(popup_wnd_proc));

        let hwnd = CreateWindowExW(
            0,
            class_name.as_ptr(),
            title.as_ptr(),
            WS_OVERLAPPEDWINDOW,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            960,
            720,
            null_mut(),
            null_mut(),
            crate::get_app_instance(),
            null_mut(),
        );
        if hwnd.is_null() {
            log_info!("Failed to create settings window");
            return;
        }

        let dpi = GetDpiForWindow(hwnd);
        let scale = if dpi > 0 { dpi as f32 / 96.0 } else { 1.0 };

        // 960x720 dips, centered on the primary monitor.
        let w = (960.0 * scale).round() as i32;
        let h = (720.0 * scale).round() as i32;
        let sw = GetSystemMetrics(SM_CXSCREEN);
        let sh = GetSystemMetrics(SM_CYSCREEN);
        SetWindowPos(
            hwnd,
            null_mut(),
            ((sw - w) / 2).max(0),
            ((sh - h) / 2).max(0),
            w,
            h,
            SWP_NOZORDER | SWP_NOACTIVATE,
        );

        apply_frame_attributes(hwnd, &pal, mica);

        let fonts = controls::create_fonts(scale);
        let mut win = Win {
            hwnd,
            scale,
            fonts,
            pal,
            mica,
            state,
            layout: Layout {
                items: Vec::new(),
                content_h: 0,
                controls: Vec::new(),
            },
            nav_rects: [RECT {
                left: 0,
                top: 0,
                right: 0,
                bottom: 0,
            }; 3],
            scroll: 0,
            hover: None,
            pressed: None,
            focus: None,
            keyboard_nav: false,
            combo: None,
            tracking_leave: false,
            scrollbar_drag: None,
            client_w: 0,
            client_h: 0,
        };
        let mut rc: RECT = std::mem::zeroed();
        GetClientRect(hwnd, &mut rc);
        win.client_w = rc.right;
        win.client_h = rc.bottom;
        relayout(&mut win);

        WIN.with(|s| *s.borrow_mut() = Some(win));

        ShowWindow(hwnd, SW_SHOW);
        SetForegroundWindow(hwnd);

        let mut msg: MSG = std::mem::zeroed();
        while windows_sys::Win32::UI::WindowsAndMessaging::GetMessageW(&mut msg, null_mut(), 0, 0)
            > 0
        {
            TranslateMessage(&msg);
            windows_sys::Win32::UI::WindowsAndMessaging::DispatchMessageW(&msg);
        }

        // Teardown (WM_DESTROY already stopped any capture hook).
        let win = WIN.with(|s| s.borrow_mut().take());
        if let Some(win) = win {
            controls::delete_fonts(&win.fonts);
        }
    }
}

unsafe fn register_class(
    name: &str,
    proc: Option<unsafe extern "system" fn(HWND, u32, WPARAM, LPARAM) -> LRESULT>,
) {
    let class_name = encode_wide(name);
    let wc = WNDCLASSEXW {
        cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
        style: CS_HREDRAW | CS_VREDRAW,
        lpfnWndProc: proc,
        cbClsExtra: 0,
        cbWndExtra: 0,
        hInstance: crate::get_app_instance(),
        hIcon: null_mut(),
        hCursor: LoadCursorW(null_mut(), IDC_ARROW),
        hbrBackground: null_mut(),
        lpszMenuName: std::ptr::null(),
        lpszClassName: class_name.as_ptr(),
        hIconSm: null_mut(),
    };
    RegisterClassExW(&wc);
}

unsafe fn apply_frame_attributes(hwnd: HWND, pal: &Palette, mica: bool) {
    let dark: i32 = if pal.light { 0 } else { 1 };
    DwmSetWindowAttribute(
        hwnd,
        DWMWA_USE_IMMERSIVE_DARK_MODE as _,
        &dark as *const _ as _,
        std::mem::size_of::<i32>() as u32,
    );
    if mica && !pal.high_contrast {
        let margins = MARGINS {
            cxLeftWidth: -1,
            cxRightWidth: -1,
            cyTopHeight: -1,
            cyBottomHeight: -1,
        };
        DwmExtendFrameIntoClientArea(hwnd, &margins);
        let backdrop: i32 = 2; // DWMSBT_MAINWINDOW (Mica)
        DwmSetWindowAttribute(
            hwnd,
            DWMWA_SYSTEMBACKDROP_TYPE as _,
            &backdrop as *const _ as _,
            std::mem::size_of::<i32>() as u32,
        );
    }
}

// ---------------------------------------------------------------------------
// Layout
// ---------------------------------------------------------------------------

fn px_of(scale: f32) -> impl Fn(i32) -> i32 {
    move |v: i32| (v as f32 * scale).round() as i32
}

fn relayout(win: &mut Win) {
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
        let screen = GetDC(null_mut());
        let hdc = CreateCompatibleDC(screen);
        let body = win.fonts.body;
        let caption = win.fonts.caption;
        win.layout = pages::layout(
            &params,
            |s| controls::measure_text(hdc, body, s),
            |s| controls::measure_text(hdc, caption, s),
        );
        DeleteDC(hdc);
        ReleaseDC(null_mut(), screen);
    }

    clamp_scroll(win);
}

fn viewport_h(win: &Win) -> i32 {
    win.client_h
}

fn clamp_scroll(win: &mut Win) {
    let max = (win.layout.content_h - viewport_h(win)).max(0);
    win.scroll = win.scroll.clamp(0, max);
}

/// Focus order: the three nav items, then the content controls.
fn focus_len(win: &Win) -> usize {
    3 + win.layout.controls.len()
}

fn focused_control(win: &Win) -> Option<ControlId> {
    match win.focus {
        Some(i) if i < 3 => Some(ControlId::Nav(Page::ALL[i])),
        Some(i) => win.layout.controls.get(i - 3).map(|(id, _)| *id),
        None => None,
    }
}

fn ensure_focus_visible(win: &mut Win) {
    let px = px_of(win.scale);
    if let Some(i) = win.focus {
        if i >= 3 {
            if let Some((_, rect)) = win.layout.controls.get(i - 3) {
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

// ---------------------------------------------------------------------------
// Hit testing
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq)]
enum HitTarget {
    Control(ControlId),
    ScrollThumb,
    ScrollTrack,
    None,
}

fn pt_in(r: &RECT, x: i32, y: i32) -> bool {
    x >= r.left && x < r.right && y >= r.top && y < r.bottom
}

/// Scrollbar geometry: (track, thumb) in window coordinates, when scrollable.
fn scrollbar_rects(win: &Win) -> Option<(RECT, RECT)> {
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

fn hit_test(win: &Win, x: i32, y: i32) -> HitTarget {
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

// ---------------------------------------------------------------------------
// Rendering
// ---------------------------------------------------------------------------

unsafe fn on_paint(hwnd: HWND) {
    let mut ps: PAINTSTRUCT = std::mem::zeroed();
    let hdc = BeginPaint(hwnd, &mut ps);
    WIN.with(|s| {
        if let Ok(borrow) = s.try_borrow() {
            if let Some(win) = borrow.as_ref() {
                controls::paint_surface(hdc, win.client_w, win.client_h, &win.pal, |mem| {
                    draw_all(mem, win);
                });
            }
        }
    });
    EndPaint(hwnd, &ps);
}

unsafe fn draw_all(hdc: windows_sys::Win32::Graphics::Gdi::HDC, win: &Win) {
    let px = px_of(win.scale);
    let pal = &win.pal;
    let fonts = &win.fonts;
    let scale_px = px(1).max(1);

    // Nav rail.
    for (i, page) in Page::ALL.iter().enumerate() {
        let r = win.nav_rects[i];
        let selected = win.state.page == *page;
        let hovered = win.hover == Some(ControlId::Nav(*page));
        if selected || hovered {
            controls::fill_round(
                hdc,
                &r,
                px(4),
                if selected { pal.card } else { pal.card_hover },
                if selected {
                    pal.card_stroke
                } else {
                    pal.card_hover
                },
            );
        }
        if selected {
            let pill = RECT {
                left: r.left,
                top: r.top + (r.bottom - r.top - px(16)) / 2,
                right: r.left + px(3),
                bottom: r.top + (r.bottom - r.top + px(16)) / 2,
            };
            controls::fill_round(hdc, &pill, px(3), pal.accent, pal.accent);
        }
        let glyph_rect = RECT {
            left: r.left + px(10),
            top: r.top,
            right: r.left + px(38),
            bottom: r.bottom,
        };
        controls::draw_glyph_in(
            hdc,
            fonts.glyph,
            pal.text,
            &glyph_rect,
            page.glyph(),
            DT_SINGLELINE | DT_VCENTER | windows_sys::Win32::Graphics::Gdi::DT_CENTER,
        );
        let text_rect = RECT {
            left: r.left + px(44),
            top: r.top,
            right: r.right - px(8),
            bottom: r.bottom,
        };
        controls::draw_text_in(
            hdc,
            if selected {
                fonts.body_strong
            } else {
                fonts.body
            },
            pal.text,
            &text_rect,
            page.nav_label(),
            DT_SINGLELINE | DT_VCENTER | DT_LEFT | DT_END_ELLIPSIS,
        );
        if win.keyboard_nav && focused_control(win) == Some(ControlId::Nav(*page)) {
            controls::draw_focus_ring(hdc, &r, pal, scale_px);
        }
    }

    // Content items (content-space rects shifted by scroll).
    let shift = |r: &RECT| RECT {
        left: r.left,
        top: r.top - win.scroll,
        right: r.right,
        bottom: r.bottom - win.scroll,
    };
    let visible = |r: &RECT| r.bottom - win.scroll > 0 && r.top - win.scroll < win.client_h;

    let focused = if win.keyboard_nav {
        focused_control(win)
    } else {
        None
    };
    let vis_for = |id: ControlId| Vis {
        hover: win.hover == Some(id),
        pressed: win.pressed == Some(id),
        focus: focused == Some(id),
    };

    for item in &win.layout.items {
        match item {
            LaidItem::Title(r, text) => {
                if visible(r) {
                    controls::draw_text_in(
                        hdc,
                        fonts.title,
                        pal.text,
                        &shift(r),
                        text,
                        DT_SINGLELINE | DT_VCENTER | DT_LEFT,
                    );
                }
            }
            LaidItem::Subtitle(r, text) => {
                if visible(r) {
                    controls::draw_text_in(
                        hdc,
                        fonts.subtitle,
                        pal.text,
                        &shift(r),
                        text,
                        DT_SINGLELINE | DT_VCENTER | DT_LEFT,
                    );
                }
            }
            LaidItem::Banner(r) => {
                if visible(r) {
                    draw_banner(hdc, win, &shift(r));
                }
            }
            LaidItem::FooterText(r) => {
                if visible(r) {
                    controls::draw_text_in(
                        hdc,
                        fonts.caption,
                        pal.text_dim,
                        &shift(r),
                        "Changes are saved automatically",
                        DT_SINGLELINE | DT_VCENTER | DT_LEFT,
                    );
                }
            }
            LaidItem::FooterButton(r) => {
                if visible(r) {
                    controls::draw_button(
                        hdc,
                        &shift(r),
                        "Reset Defaults",
                        false,
                        vis_for(ControlId::BtnReset),
                        pal,
                        fonts,
                        scale_px,
                    );
                }
            }
            LaidItem::Card(card) => {
                if !visible(&card.rect) {
                    continue;
                }
                let r = shift(&card.rect);
                let clickable = card.click.is_some();
                let card_vis = card.click.map(vis_for).unwrap_or_default();
                let fill = if clickable && card_vis.pressed {
                    pal.card_pressed
                } else if clickable && card_vis.hover {
                    pal.card_hover
                } else {
                    pal.card
                };
                controls::fill_round(hdc, &r, px(4), fill, pal.card_stroke);
                if card_vis.focus {
                    controls::draw_focus_ring(hdc, &r, pal, scale_px);
                }

                // Leading glyph.
                let glyph_rect = RECT {
                    left: r.left + px(16),
                    top: r.top,
                    right: r.left + px(44),
                    bottom: r.bottom,
                };
                controls::draw_glyph_in(
                    hdc,
                    fonts.glyph,
                    pal.text,
                    &glyph_rect,
                    card.glyph,
                    DT_SINGLELINE | DT_VCENTER | windows_sys::Win32::Graphics::Gdi::DT_CENTER,
                );

                // Text block, clipped at the trailing control.
                let trail_left = match &card.trailing {
                    LaidTrailing::None => r.right - px(16),
                    LaidTrailing::Toggle(_, tr) => tr.left,
                    LaidTrailing::Button(_, _, tr) => tr.left,
                    LaidTrailing::Buttons(list) => list
                        .iter()
                        .map(|(_, _, tr)| tr.left)
                        .min()
                        .unwrap_or(r.right),
                    LaidTrailing::Hotkey(_, tr) => tr.left,
                    LaidTrailing::Combo(tr) => tr.left,
                    LaidTrailing::Hero { pill, .. } => pill.left,
                };
                let text_right = trail_left - px(12);
                if card.header.is_empty() {
                    let text_rect = RECT {
                        left: r.left + px(56),
                        top: r.top,
                        right: text_right,
                        bottom: r.bottom,
                    };
                    controls::draw_text_in(
                        hdc,
                        fonts.caption,
                        pal.text_dim,
                        &text_rect,
                        &card.desc,
                        DT_SINGLELINE | DT_VCENTER | DT_LEFT | DT_END_ELLIPSIS,
                    );
                } else {
                    let header_rect = RECT {
                        left: r.left + px(56),
                        top: r.top + px(14),
                        right: text_right,
                        bottom: r.top + px(36),
                    };
                    controls::draw_text_in(
                        hdc,
                        fonts.body,
                        pal.text,
                        &header_rect,
                        &card.header,
                        DT_SINGLELINE | DT_LEFT | DT_END_ELLIPSIS,
                    );
                    let desc_rect = RECT {
                        left: r.left + px(56),
                        top: r.top + px(36),
                        right: text_right,
                        bottom: r.bottom - px(8),
                    };
                    controls::draw_text_in(
                        hdc,
                        fonts.caption,
                        pal.text_dim,
                        &desc_rect,
                        &card.desc,
                        DT_SINGLELINE | DT_LEFT | DT_END_ELLIPSIS,
                    );
                }

                // Trailing control.
                match &card.trailing {
                    LaidTrailing::None => {}
                    LaidTrailing::Toggle(id, tr) => {
                        let on = match id {
                            ControlId::ToggleShowAll => win.state.config.show_all_taskbar,
                            ControlId::ToggleWinTab => win.state.config.intercept_win_tab,
                            ControlId::ToggleAutoRestore => {
                                win.state.config.auto_restore_workspaces
                            }
                            ControlId::ToggleAutostart => win.state.autostart,
                            _ => false,
                        };
                        controls::draw_toggle(hdc, &shift(tr), on, vis_for(*id), pal, scale_px);
                    }
                    LaidTrailing::Button(id, label, tr) => {
                        controls::draw_button(
                            hdc,
                            &shift(tr),
                            label,
                            false,
                            vis_for(*id),
                            pal,
                            fonts,
                            scale_px,
                        );
                    }
                    LaidTrailing::Buttons(list) => {
                        for (id, label, tr) in list {
                            controls::draw_button(
                                hdc,
                                &shift(tr),
                                label,
                                false,
                                vis_for(*id),
                                pal,
                                fonts,
                                scale_px,
                            );
                        }
                    }
                    LaidTrailing::Hotkey(target, tr) => {
                        let capturing = win.state.capturing == Some(*target);
                        let label = if capturing {
                            "Press keys...".to_string()
                        } else {
                            target.display(&win.state.config)
                        };
                        controls::draw_field(
                            hdc,
                            &shift(tr),
                            &label,
                            capturing,
                            false,
                            vis_for(ControlId::Hotkey(*target)),
                            pal,
                            fonts,
                            scale_px,
                        );
                    }
                    LaidTrailing::Combo(tr) => {
                        controls::draw_field(
                            hdc,
                            &shift(tr),
                            win.state.theme_pref.label(),
                            false,
                            true,
                            vis_for(ControlId::ComboTheme),
                            pal,
                            fonts,
                            scale_px,
                        );
                    }
                    LaidTrailing::Hero { pill, btn } => {
                        let (text, dot, bg) = if win.state.daemon_running {
                            ("Daemon Active & Running", pal.success, pal.success_bg)
                        } else {
                            ("Daemon Stopped", pal.critical, pal.critical_bg)
                        };
                        controls::draw_pill(hdc, &shift(pill), text, dot, bg, pal, fonts, scale_px);
                        controls::draw_button(
                            hdc,
                            &shift(btn),
                            "Reload Daemon",
                            false,
                            vis_for(ControlId::BtnReload),
                            pal,
                            fonts,
                            scale_px,
                        );
                    }
                }
            }
        }
    }

    // Overlay scrollbar.
    if let Some((_track, thumb)) = scrollbar_rects(win) {
        let active = win.scrollbar_drag.is_some();
        controls::fill_round(
            hdc,
            &thumb,
            px(4),
            if active { pal.text_dim } else { pal.ctl_stroke },
            if active { pal.text_dim } else { pal.ctl_stroke },
        );
    }
}

unsafe fn draw_banner(hdc: windows_sys::Win32::Graphics::Gdi::HDC, win: &Win, r: &RECT) {
    let Some(banner) = &win.state.banner else {
        return;
    };
    let px = px_of(win.scale);
    let pal = &win.pal;
    let (bg, fg, glyph) = if banner.success {
        (pal.success_bg, pal.success, 0xE930u16) // Completed
    } else {
        (pal.critical_bg, pal.critical, 0xEA39u16) // ErrorBadge
    };
    controls::fill_round(hdc, r, px(4), bg, pal.card_stroke);
    let glyph_rect = RECT {
        left: r.left + px(14),
        top: r.top,
        right: r.left + px(40),
        bottom: r.bottom,
    };
    controls::draw_glyph_in(
        hdc,
        win.fonts.glyph,
        fg,
        &glyph_rect,
        glyph,
        DT_SINGLELINE | DT_VCENTER | windows_sys::Win32::Graphics::Gdi::DT_CENTER,
    );
    let title_w = {
        let screen = GetDC(null_mut());
        let mem = CreateCompatibleDC(screen);
        let w = controls::measure_text(mem, win.fonts.body_strong, &banner.title);
        DeleteDC(mem);
        ReleaseDC(null_mut(), screen);
        w
    };
    let title_rect = RECT {
        left: r.left + px(48),
        top: r.top,
        right: r.left + px(48) + title_w + px(4),
        bottom: r.bottom,
    };
    controls::draw_text_in(
        hdc,
        win.fonts.body_strong,
        pal.text,
        &title_rect,
        &banner.title,
        DT_SINGLELINE | DT_VCENTER | DT_LEFT,
    );
    let msg_rect = RECT {
        left: title_rect.right + px(10),
        top: r.top,
        right: r.right - px(12),
        bottom: r.bottom,
    };
    controls::draw_text_in(
        hdc,
        win.fonts.body,
        pal.text_dim,
        &msg_rect,
        &banner.message,
        DT_SINGLELINE | DT_VCENTER | DT_LEFT | DT_END_ELLIPSIS,
    );
}

// ---------------------------------------------------------------------------
// Actions
// ---------------------------------------------------------------------------

/// Post-action housekeeping: banner timer, relayout (banner/content changes
/// shift rects), repaint.
unsafe fn after_action(win: &mut Win) {
    if win.state.banner.is_some() {
        SetTimer(win.hwnd, TIMER_BANNER, 4000, None);
    }
    relayout(win);
    InvalidateRect(win.hwnd, std::ptr::null(), 0);
}

unsafe fn stop_recording(win: &mut Win) {
    if win.state.capturing.is_some() {
        recorder::stop_capture();
        win.state.capturing = None;
        InvalidateRect(win.hwnd, std::ptr::null(), 0);
    }
}

unsafe fn activate(win: &mut Win, id: ControlId) {
    // Any activation while recording cancels the recording first.
    if win.state.capturing.is_some() && !matches!(id, ControlId::Hotkey(_)) {
        stop_recording(win);
    }
    match id {
        ControlId::Nav(page) | ControlId::NavCard(page, _) => {
            if win.state.page != page {
                win.state.page = page;
                win.scroll = 0;
                win.hover = None;
                relayout(win);
                InvalidateRect(win.hwnd, std::ptr::null(), 0);
            }
        }
        ControlId::ToggleShowAll => {
            win.state.config.show_all_taskbar = !win.state.config.show_all_taskbar;
            win.state.autosave("Taskbar visibility mode updated");
            after_action(win);
        }
        ControlId::ToggleWinTab => {
            win.state.config.intercept_win_tab = !win.state.config.intercept_win_tab;
            win.state.autosave("Intercept Win + Tab preference updated");
            after_action(win);
        }
        ControlId::ToggleAutoRestore => {
            win.state.config.auto_restore_workspaces = !win.state.config.auto_restore_workspaces;
            win.state.autosave("Auto-restore preference updated");
            after_action(win);
        }
        ControlId::ToggleAutostart => {
            win.state.toggle_autostart();
            after_action(win);
        }
        ControlId::ComboTheme => {
            if win.combo.is_some() {
                close_combo(win);
            } else {
                open_combo(win);
            }
        }
        ControlId::BtnReload => {
            win.state.refresh_daemon_status();
            relayout(win);
            InvalidateRect(win.hwnd, std::ptr::null(), 0);
        }
        ControlId::BtnCapture => {
            if win.state.request_capture() {
                // Capture is asynchronous: give the daemon ~300ms to write
                // the captured rules, then re-read the file.
                SetTimer(win.hwnd, TIMER_CAPTURE, 300, None);
            } else {
                win.state.show_banner(
                    "Daemon Not Running",
                    "Start the WinSpaces daemon to capture window layouts.",
                    false,
                );
                after_action(win);
            }
        }
        ControlId::BtnRestore => {
            win.state.request_restore();
            after_action(win);
        }
        ControlId::BtnReset => {
            win.state.reset_defaults();
            after_action(win);
        }
        ControlId::Hotkey(target) => {
            if win.state.capturing == Some(target) {
                stop_recording(win);
            } else {
                recorder::stop_capture();
                win.state.capture_hook = recorder::start_capture(win.hwnd);
                win.state.capturing = Some(target);
                InvalidateRect(win.hwnd, std::ptr::null(), 0);
            }
        }
        ControlId::RuleDelete(index) => {
            win.state.delete_rule(index);
            after_action(win);
        }
    }
}

unsafe fn handle_recorder_key(win: &mut Win, vk: u32) {
    let Some(target) = win.state.capturing else {
        return;
    };
    match recorder::translate_current(vk) {
        recorder::Outcome::Ignore => {}
        recorder::Outcome::Cancel => {
            stop_recording(win);
        }
        recorder::Outcome::Commit(hk) => {
            recorder::stop_capture();
            win.state.capturing = None;
            win.state.set_hotkey(target, hk);
            after_action(win);
        }
    }
}

// ---------------------------------------------------------------------------
// Combo popup
// ---------------------------------------------------------------------------

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

unsafe fn open_combo(win: &mut Win) {
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
        crate::get_app_instance(),
        null_mut(),
    );
    if hwnd.is_null() {
        return;
    }
    let dark: i32 = if win.pal.light { 0 } else { 1 };
    DwmSetWindowAttribute(
        hwnd,
        DWMWA_USE_IMMERSIVE_DARK_MODE as _,
        &dark as *const _ as _,
        std::mem::size_of::<i32>() as u32,
    );
    let corner: u32 = 2; // DWMWCP_ROUND
    DwmSetWindowAttribute(
        hwnd,
        33, // DWMWA_WINDOW_CORNER_PREFERENCE
        &corner as *const _ as _,
        std::mem::size_of::<u32>() as u32,
    );
    ShowWindow(
        hwnd,
        windows_sys::Win32::UI::WindowsAndMessaging::SW_SHOWNOACTIVATE,
    );
    win.combo = Some(ComboPopup {
        hwnd,
        width,
        item_h,
        pad,
        hover: Some(win.state.theme_pref.index()),
    });
}

unsafe fn close_combo(win: &mut Win) {
    if let Some(combo) = win.combo.take() {
        DestroyWindow(combo.hwnd);
    }
}

unsafe extern "system" fn popup_wnd_proc(
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
            WIN.with(|s| {
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

unsafe fn draw_combo(hdc: windows_sys::Win32::Graphics::Gdi::HDC, win: &Win, combo: &ComboPopup) {
    let px = px_of(win.scale);
    let pal = &win.pal;
    let h = combo.item_h * ThemePref::ALL.len() as i32 + combo.pad * 2;
    // Opaque popup surface (no backdrop behind a NOACTIVATE popup).
    let mut opaque = pal.clone();
    opaque.base_alpha = 255;
    let flyout = if pal.light { 0xF9 } else { 0x2C };
    opaque.base_r = flyout;
    opaque.base_g = flyout;
    opaque.base_b = flyout;
    controls::paint_surface(hdc, combo.width, h, &opaque, |mem| {
        for (i, pref) in ThemePref::ALL.iter().enumerate() {
            let top = combo.pad + i as i32 * combo.item_h;
            let r = RECT {
                left: px(4),
                top,
                right: combo.width - px(4),
                bottom: top + combo.item_h,
            };
            if combo.hover == Some(i) {
                controls::fill_round(mem, &r, px(4), pal.ctl_hover, pal.ctl_hover);
            }
            if win.state.theme_pref.index() == i {
                let pill = RECT {
                    left: px(4),
                    top: top + (combo.item_h - px(16)) / 2,
                    right: px(4) + px(3),
                    bottom: top + (combo.item_h + px(16)) / 2,
                };
                controls::fill_round(mem, &pill, px(3), pal.accent, pal.accent);
            }
            let text_rect = RECT {
                left: px(16),
                top,
                right: combo.width - px(8),
                bottom: top + combo.item_h,
            };
            controls::draw_text_in(
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

unsafe fn commit_combo(win: &mut Win, index: usize) {
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

// ---------------------------------------------------------------------------
// Window procedure
// ---------------------------------------------------------------------------

unsafe fn wheel_scroll_lines() -> i32 {
    let mut lines: u32 = 3;
    SystemParametersInfoW(SPI_GETWHEELSCROLLLINES, 0, &mut lines as *mut _ as _, 0);
    lines.clamp(1, 20) as i32
}

unsafe fn on_key_down(win: &mut Win, vk: u32) {
    let px = px_of(win.scale);

    // Combo keyboard model while the dropdown is open.
    if win.combo.is_some() {
        match vk as u16 {
            VK_ESCAPE => close_combo(win),
            VK_UP | VK_DOWN => {
                if let Some(combo) = win.combo.as_mut() {
                    let len = ThemePref::ALL.len();
                    let cur = combo.hover.unwrap_or(win.state.theme_pref.index());
                    let next = if vk as u16 == VK_DOWN {
                        (cur + 1) % len
                    } else {
                        (cur + len - 1) % len
                    };
                    combo.hover = Some(next);
                    InvalidateRect(combo.hwnd, std::ptr::null(), 0);
                }
            }
            VK_RETURN | VK_SPACE => {
                let idx = win
                    .combo
                    .as_ref()
                    .and_then(|c| c.hover)
                    .unwrap_or(win.state.theme_pref.index());
                commit_combo(win, idx);
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

unsafe extern "system" fn settings_wnd_proc(
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
                    if let HitTarget::Control(ControlId::ComboTheme) = hit_test(win, x, y) {
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
            with_win(|win| commit_combo(win, wparam.min(ThemePref::ALL.len() - 1)));
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
            if (wparam & 0xFFFF) as u32 == windows_sys::Win32::UI::WindowsAndMessaging::WA_INACTIVE
            {
                with_win(|win| {
                    stop_recording(win);
                    if win.combo.is_some() {
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
