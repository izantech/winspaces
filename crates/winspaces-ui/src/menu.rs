//! Custom Windows 11-style tray context menu: a hand-drawn `WS_POPUP` window
//! with a DWM acrylic backdrop, rounded corners, Segoe Fluent Icons glyphs
//! and hover highlights. Pure GDI against already-linked system libraries —
//! no new dependencies, no GPU device, nothing resident while closed.
//!
//! Technique (proven by PowerToys' AltWindowCycle and the wcpopup crate):
//! `WS_EX_NOACTIVATE` popup + `DwmExtendFrameIntoClientArea(-1)` +
//! `DWMWA_SYSTEMBACKDROP_TYPE = DWMSBT_TRANSIENTWINDOW`, painted through a
//! 32bpp premultiplied DIB whose alpha channel is managed manually so the
//! acrylic shows through the background tint. Light dismiss combines mouse
//! capture on the root window with owner-deactivation forwarding; keyboard
//! input arrives re-posted from the daemon's existing low-level hook.

mod input;
mod layout;
pub mod legacy;
mod render;

use crate::theme::{menu_theme, MenuTheme};
use layout::hit_test;
use std::cell::RefCell;
use std::ptr::null_mut;
use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows_sys::Win32::Graphics::Gdi::{
    BeginPaint, CreatePen, CreateSolidBrush, DeleteObject, EndPaint, HBRUSH, HFONT, HPEN,
    PAINTSTRUCT, PS_SOLID,
};
use windows_sys::Win32::Graphics::Gdi::{MonitorFromPoint, MONITOR_DEFAULTTONEAREST};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    ReleaseCapture, SetCapture, VK_DOWN, VK_ESCAPE, VK_LEFT, VK_LMENU, VK_LWIN, VK_MENU, VK_RETURN,
    VK_RIGHT, VK_RMENU, VK_RWIN, VK_SPACE, VK_UP,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, CreateWindowExW, DefWindowProcW, DestroyWindow, KillTimer, LoadCursorW,
    PostMessageW, SetCursor, SetForegroundWindow, SetWindowsHookExW, ShowWindow,
    UnhookWindowsHookEx, HHOOK, IDC_ARROW, MSLLHOOKSTRUCT, SW_SHOWNOACTIVATE, WH_MOUSE_LL, WM_APP,
    WM_CAPTURECHANGED, WM_DESTROY, WM_ERASEBKGND, WM_KEYDOWN, WM_LBUTTONDOWN, WM_LBUTTONUP,
    WM_MBUTTONDOWN, WM_MOUSEMOVE, WM_PAINT, WM_RBUTTONDOWN, WM_RBUTTONUP, WM_TIMER,
    WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_POPUP,
};
use winspaces_common::log_info;
use winspaces_win32::dpi::{px, scale_for_point, work_area};
use winspaces_win32::dwm::{
    extend_frame_full, set_backdrop, set_dark_mode, set_round_corners, Backdrop,
};
use winspaces_win32::gdi::font::{create_font, FACE_ICONS, FACE_TEXT};
use winspaces_win32::module::{app_instance, win_build};
use winspaces_win32::text::encode_wide;
use winspaces_win32::window_class::register_class;

const MENU_CLASS_NAME: &str = "WinSpacesMenu";

/// Posted (never sent) so teardown always runs from the message loop, outside
/// any hook callback or state borrow.
const WM_MENU_CLOSE: u32 = WM_APP + 41;

const TIMER_OPEN_SUB: usize = 1;
const TIMER_CLOSE_SUB: usize = 2;

// Layout metrics at 96 dpi, scaled by the target monitor's DPI.
const ROW_H: i32 = 38;
const HEADER_H: i32 = 30;
const SEP_H: i32 = 9;
const PAD_V: i32 = 6;
const HOVER_INSET: i32 = 5;
const HOVER_RADIUS: i32 = 8;
const ICON_X: i32 = 14;
const TEXT_X: i32 = 46;
const LABEL_GAP: i32 = 28;
const RIGHT_PAD: i32 = 16;
const CHEVRON_W: i32 = 22;
const MIN_W: i32 = 230;
const MAX_W: i32 = 420;

/// Background opacity over the acrylic backdrop (255 = opaque fallback).
const BG_ALPHA: u32 = 232;

/// Flyout palette, matching the Windows 11 flyout look in both modes.
/// Resolved on every menu open from the shared theme preference
/// (`crate::theme`: App Theme selector, falling back to the OS apps mode),
/// so the menu follows live theme changes without any hooks. `MenuTheme`
/// itself lives in `crate::theme` as a documented sibling of the settings
/// window's `Palette` so the two surfaces cannot drift apart.
pub enum MenuEntry {
    Header(String),
    Separator,
    Item(MenuItemData),
}

pub struct MenuItemData {
    pub id: usize,
    pub glyph: Option<u16>,
    pub label: String,
    pub shortcut: Option<String>,
    pub checked: bool,
    pub submenu: Option<Vec<MenuEntry>>,
}

impl MenuEntry {
    pub(crate) fn is_selectable(&self) -> bool {
        matches!(self, MenuEntry::Item(_))
    }
}

#[derive(Clone, Copy)]
pub(crate) struct Row {
    pub(crate) top: i32,
    pub(crate) height: i32,
}

pub(crate) struct MenuWindow {
    pub(crate) hwnd: HWND,
    /// Screen position of the window's top-left corner.
    pub(crate) x: i32,
    pub(crate) y: i32,
    pub(crate) width: i32,
    pub(crate) height: i32,
    pub(crate) entries: Vec<MenuEntry>,
    pub(crate) rows: Vec<Row>,
    pub(crate) hover: Option<usize>,
    pub(crate) sel: Option<usize>,
}

pub(crate) struct Fonts {
    pub(crate) text: HFONT,
    pub(crate) small: HFONT,
    pub(crate) glyph: HFONT,
    pub(crate) glyph_small: HFONT,
}

/// Theme-colored GDI objects reused across paints. The theme is resolved once
/// per open and never changes while the menu is up, so these are created next
/// to the fonts and destroyed with them in `close_menu` — three fewer
/// create/delete pairs per hover repaint, nothing retained past close.
pub(crate) struct Paints {
    pub(crate) hover_brush: HBRUSH,
    pub(crate) hover_pen: HPEN,
    pub(crate) sep_brush: HBRUSH,
}

pub(crate) struct MenuState {
    pub(crate) owner: HWND,
    pub(crate) root: MenuWindow,
    pub(crate) sub: Option<MenuWindow>,
    pub(crate) sub_parent: Option<usize>,
    pub(crate) pending_sub: Option<usize>,
    pub(crate) fonts: Fonts,
    pub(crate) paints: Paints,
    pub(crate) scale: f32,
    pub(crate) acrylic: bool,
    pub(crate) light: bool,
    pub(crate) theme: MenuTheme,
    /// `SPI_GETMENUSHOWDELAY`, read once per open instead of per mouse-move.
    pub(crate) sub_delay_ms: u32,
    /// Work area of the monitor the menu opened on, for submenu flipping.
    pub(crate) work: RECT,
    /// Low-level mouse hook alive only while the menu is open: `SetCapture`
    /// cannot be relied on (it needs the foreground thread), so this is the
    /// authoritative outside-click dismisser.
    pub(crate) mouse_hook: HHOOK,
}

thread_local! {
    pub(crate) static MENU_STATE: RefCell<Option<MenuState>> = const { RefCell::new(None) };
}

pub fn is_menu_open() -> bool {
    MENU_STATE.with(|s| s.try_borrow().map(|st| st.is_some()).unwrap_or(false))
}

pub(crate) fn root_hwnd() -> HWND {
    MENU_STATE.with(|s| {
        s.try_borrow()
            .ok()
            .and_then(|st| st.as_ref().map(|st| st.root.hwnd))
            .unwrap_or(null_mut())
    })
}

/// Called from the daemon's low-level keyboard hook while the menu is open.
/// Returns true when the key was consumed (the hook must swallow it).
pub fn forward_key(vk: u32) -> bool {
    let hwnd = root_hwnd();
    if hwnd.is_null() {
        return false;
    }
    match vk as u16 {
        VK_ESCAPE | VK_UP | VK_DOWN | VK_LEFT | VK_RIGHT | VK_RETURN | VK_SPACE => unsafe {
            PostMessageW(hwnd, WM_KEYDOWN, vk as usize, 0);
            true
        },
        VK_LWIN | VK_RWIN | VK_MENU | VK_LMENU | VK_RMENU => unsafe {
            // Win/Alt leave menu context, like native menus. Swallowing the
            // keydown also keeps the Start menu from opening on release.
            PostMessageW(hwnd, WM_MENU_CLOSE, 0, 0);
            true
        },
        _ => false,
    }
}

/// Called from the message window's WM_ACTIVATE handler: losing foreground
/// (Alt-Tab, another app activated) light-dismisses the menu.
pub fn handle_owner_deactivate() {
    let hwnd = root_hwnd();
    if !hwnd.is_null() {
        unsafe {
            PostMessageW(hwnd, WM_MENU_CLOSE, 0, 0);
        }
    }
}

/// LL mouse hook: a button press outside every menu window light-dismisses.
/// The press is swallowed (native menus do the same); keep the callback
/// bounded — teardown happens on the message loop via `WM_MENU_CLOSE`.
unsafe extern "system" fn mouse_ll_proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code >= 0 {
        let msg = wparam as u32;
        if msg == WM_LBUTTONDOWN || msg == WM_RBUTTONDOWN || msg == WM_MBUTTONDOWN {
            let info = &*(lparam as *const MSLLHOOKSTRUCT);
            let pt = info.pt;
            let dismiss = MENU_STATE.with(|s| {
                s.try_borrow()
                    .ok()
                    .and_then(|b| {
                        b.as_ref()
                            .map(|state| hit_test(state, pt) == layout::Hit::Outside)
                    })
                    .unwrap_or(false)
            });
            if dismiss {
                let hwnd = root_hwnd();
                if !hwnd.is_null() {
                    PostMessageW(hwnd, WM_MENU_CLOSE, 0, 0);
                }
                return 1;
            }
        }
    }
    CallNextHookEx(null_mut(), code, wparam, lparam)
}

pub fn close_menu() {
    unsafe {
        let state = MENU_STATE.with(|s| s.borrow_mut().take());
        if let Some(state) = state {
            if !state.mouse_hook.is_null() {
                UnhookWindowsHookEx(state.mouse_hook);
            }
            KillTimer(state.root.hwnd, TIMER_OPEN_SUB);
            KillTimer(state.root.hwnd, TIMER_CLOSE_SUB);
            ReleaseCapture();
            if let Some(sub) = &state.sub {
                DestroyWindow(sub.hwnd);
            }
            DestroyWindow(state.root.hwnd);
            DeleteObject(state.fonts.text as _);
            DeleteObject(state.fonts.small as _);
            DeleteObject(state.fonts.glyph as _);
            DeleteObject(state.fonts.glyph_small as _);
            DeleteObject(state.paints.hover_brush as _);
            DeleteObject(state.paints.hover_pen as _);
            DeleteObject(state.paints.sep_brush as _);
        }
    }
}

unsafe fn create_paints(theme: &MenuTheme) -> Paints {
    Paints {
        hover_brush: CreateSolidBrush(theme.hover),
        hover_pen: CreatePen(PS_SOLID, 1, theme.hover),
        sep_brush: CreateSolidBrush(theme.separator),
    }
}

unsafe fn delete_fonts(fonts: &Fonts) {
    DeleteObject(fonts.text as _);
    DeleteObject(fonts.small as _);
    DeleteObject(fonts.glyph as _);
    DeleteObject(fonts.glyph_small as _);
}

unsafe fn create_fonts(scale: f32) -> Fonts {
    // Kit's create_font passes `height` straight through to CreateFontW; the
    // menu's own convention (a NEGATED char height) is the caller's to keep.
    Fonts {
        text: create_font(FACE_TEXT, -px(scale, 15), 400),
        small: create_font(FACE_TEXT, -px(scale, 12), 400),
        glyph: create_font(FACE_ICONS, -px(scale, 16), 400),
        glyph_small: create_font(FACE_ICONS, -px(scale, 12), 400),
    }
}

/// Show the tray context menu built from `entries`.
///
/// Windows 11 gets the custom acrylic flyout below; older builds — where the
/// flyout's DWM backdrop has nothing to render against — fall back to the
/// classic system `HMENU` in [`legacy`]. Both renderers consume the exact
/// same `entries`, so the two can never drift into different menu
/// structures.
pub fn show(owner: HWND, entries: Vec<MenuEntry>, anchor: POINT) {
    if win_build() >= 22000 {
        show_custom(owner, entries, anchor);
    } else {
        legacy::show(owner, &entries, anchor);
    }
}

/// `owner` is an opaque Win32 handle, never dereferenced in Rust — it is only
/// ever forwarded to Win32 APIs (`SetForegroundWindow`, `CreateWindowExW` via
/// `create_menu_window`), so this stays a safe fn despite carrying a
/// raw-pointer-typed parameter.
#[allow(clippy::not_unsafe_ptr_arg_deref)]
fn show_custom(owner: HWND, entries: Vec<MenuEntry>, anchor: POINT) {
    unsafe {
        close_menu();

        let hmon = MonitorFromPoint(anchor, MONITOR_DEFAULTTONEAREST);
        let scale = scale_for_point(anchor);
        let work = work_area(hmon).unwrap_or(RECT {
            left: 0,
            top: 0,
            right: 0,
            bottom: 0,
        });

        let fonts = create_fonts(scale);
        let (rows, width, height) = layout::layout_window(&entries, &fonts, scale);

        // Clamp/flip against the work area; near the taskbar this opens the
        // menu upward and leftward from the cursor like native tray menus.
        // The final clamp matters: a tray click anchors inside the taskbar
        // itself, so the flipped bottom edge must be pulled back above it.
        let mut x = anchor.x;
        if x + width > work.right {
            x = anchor.x - width;
        }
        x = x.clamp(work.left, (work.right - width).max(work.left));
        let mut y = anchor.y;
        if y + height > work.bottom {
            y = anchor.y - height;
        }
        y = y.clamp(work.top, (work.bottom - height).max(work.top));

        let acrylic = win_build() >= 22621;
        let light = crate::theme::resolves_light(crate::theme::load_pref());
        let hwnd = create_menu_window(owner, x, y, width, height, acrylic, light);
        if hwnd.is_null() {
            log_info!("Failed to create custom menu window");
            delete_fonts(&fonts);
            return;
        }

        let mouse_hook = SetWindowsHookExW(
            WH_MOUSE_LL,
            Some(mouse_ll_proc),
            windows_sys::Win32::System::LibraryLoader::GetModuleHandleW(null_mut()),
            0,
        );

        let theme = menu_theme(light);
        let paints = create_paints(&theme);
        let sub_delay_ms = input::submenu_show_delay();

        MENU_STATE.with(|s| {
            *s.borrow_mut() = Some(MenuState {
                owner,
                root: MenuWindow {
                    hwnd,
                    x,
                    y,
                    width,
                    height,
                    entries,
                    rows,
                    hover: None,
                    sel: None,
                },
                sub: None,
                sub_parent: None,
                pending_sub: None,
                fonts,
                paints,
                scale,
                acrylic,
                light,
                theme,
                sub_delay_ms,
                work,
                mouse_hook,
            });
        });

        // Foreground must go to the owner before showing: the owner's
        // deactivation is one of the light-dismiss signals (KB135788 spirit).
        SetForegroundWindow(owner);
        ShowWindow(hwnd, SW_SHOWNOACTIVATE);
        SetCapture(hwnd);
        // While capture is held Windows stops sending WM_SETCURSOR, freezing
        // whatever cursor was active at click time (often the app-starting
        // spinner). Force the arrow explicitly, like wcpopup does.
        SetCursor(LoadCursorW(null_mut(), IDC_ARROW));
    }
}

pub(crate) unsafe fn create_menu_window(
    owner: HWND,
    x: i32,
    y: i32,
    width: i32,
    height: i32,
    acrylic: bool,
    light: bool,
) -> HWND {
    register_class(MENU_CLASS_NAME, Some(menu_wnd_proc));
    let class_name = encode_wide(MENU_CLASS_NAME);
    let hwnd = CreateWindowExW(
        WS_EX_TOOLWINDOW | WS_EX_TOPMOST | WS_EX_NOACTIVATE,
        class_name.as_ptr(),
        std::ptr::null(),
        WS_POPUP,
        x,
        y,
        width,
        height,
        owner,
        null_mut(),
        app_instance(),
        null_mut(),
    );
    if hwnd.is_null() {
        return hwnd;
    }

    // Also steers the tint DWM applies to the acrylic backdrop.
    set_dark_mode(hwnd, !light);
    // Win11 rounds the popup and supplies the shadow.
    set_round_corners(hwnd);
    if acrylic {
        // "Sheet of glass" frame extension is REQUIRED before the backdrop
        // attribute has any visible effect (PowerToys AltWindowCycle recipe).
        // Load-bearing order: extend_frame_full MUST run before set_backdrop.
        extend_frame_full(hwnd);
        set_backdrop(hwnd, Backdrop::Acrylic);
    }
    hwnd
}

unsafe fn on_paint(hwnd: HWND) {
    let mut ps: PAINTSTRUCT = std::mem::zeroed();
    let hdc = BeginPaint(hwnd, &mut ps);
    MENU_STATE.with(|s| {
        let borrow = s.borrow();
        let Some(state) = borrow.as_ref() else {
            return;
        };
        if hwnd == state.root.hwnd {
            render::render_window(&state.root, state, hdc, &ps.rcPaint);
        } else if let Some(sub) = &state.sub {
            if hwnd == sub.hwnd {
                render::render_window(sub, state, hdc, &ps.rcPaint);
            }
        }
    });
    EndPaint(hwnd, &ps);
}

unsafe extern "system" fn menu_wnd_proc(
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
        WM_MOUSEMOVE => {
            input::on_mouse_move();
            0
        }
        WM_LBUTTONDOWN | WM_RBUTTONDOWN => {
            input::on_mouse_down();
            0
        }
        WM_LBUTTONUP | WM_RBUTTONUP => {
            input::on_mouse_up();
            0
        }
        WM_KEYDOWN => {
            input::on_key(wparam as u32);
            0
        }
        WM_TIMER => {
            input::on_timer(wparam);
            0
        }
        WM_MENU_CLOSE => {
            close_menu();
            0
        }
        WM_CAPTURECHANGED => {
            // Losing capture (another window grabbed it) is a dismiss signal;
            // during teardown the state is already gone, making this a no-op.
            if is_menu_open() {
                close_menu();
            }
            0
        }
        WM_DESTROY => {
            // Last-resort teardown. `close_menu` is the ONLY path that unhooks
            // `WH_MOUSE_LL` and clears `MENU_STATE`, and a root window can be
            // destroyed without it — `DefWindowProc` turns an externally posted
            // `WM_CLOSE` straight into `DestroyWindow`. The window would die
            // while the state said "open", stranding two global input hooks:
            // the daemon's `WH_KEYBOARD_LL` swallows Space/arrows/Enter/Esc
            // system-wide while `is_menu_open()`, so the user loses their
            // spacebar with no visible cause and no way back short of killing
            // the daemon. Cheap insurance against a severe, silent failure.
            //
            // Two things keep this from misfiring:
            //
            // 1. Only the ROOT window tears down. Submenus share this class and
            //    are destroyed by `close_submenu` while the root legitimately
            //    stays open; tearing down there would collapse the whole menu
            //    on every hover-out.
            // 2. `try_borrow`, never `borrow`. `close_submenu` calls
            //    `DestroyWindow` while holding a mutable borrow of
            //    `MENU_STATE`, and `WM_DESTROY` is delivered synchronously
            //    inside that call — a plain `borrow()` would panic inside a
            //    wndproc. A failed borrow means we are already inside a
            //    deliberate internal operation, which is exactly when this
            //    guard should stay out of the way.
            //
            // The normal path self-cancels: `close_menu` takes the state before
            // destroying anything, so the nested `WM_DESTROY` sees `None`.
            let is_root = MENU_STATE.with(|s| {
                s.try_borrow()
                    .map(|st| st.as_ref().is_some_and(|m| m.root.hwnd == hwnd))
                    .unwrap_or(false)
            });
            if is_root {
                log_info!("Menu root window destroyed outside close_menu; running teardown");
                close_menu();
            }
            0
        }
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}
