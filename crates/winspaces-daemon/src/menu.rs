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

use crate::log_info;
use crate::tray::encode_wide;
use std::cell::RefCell;
use std::ptr::null_mut;
use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, SIZE, WPARAM};
use windows_sys::Win32::Graphics::Dwm::{
    DwmExtendFrameIntoClientArea, DwmSetWindowAttribute, DWMWA_SYSTEMBACKDROP_TYPE,
    DWMWA_USE_IMMERSIVE_DARK_MODE,
};
use windows_sys::Win32::Graphics::Gdi::{
    BeginPaint, BitBlt, CreateCompatibleDC, CreateDIBSection, CreateFontW, CreatePen,
    CreateSolidBrush, DeleteDC, DeleteObject, DrawTextW, EndPaint, FillRect, GetDC,
    GetMonitorInfoW, GetTextExtentPoint32W, InvalidateRect, MonitorFromPoint, ReleaseDC, RoundRect,
    SelectObject, SetBkMode, SetTextColor, BITMAPINFO, BITMAPINFOHEADER, BI_RGB, CLEARTYPE_QUALITY,
    CLIP_DEFAULT_PRECIS, DEFAULT_CHARSET, DEFAULT_PITCH, DIB_RGB_COLORS, DT_CENTER,
    DT_END_ELLIPSIS, DT_NOPREFIX, DT_RIGHT, DT_SINGLELINE, DT_VCENTER, HDC, HFONT, MONITORINFO,
    MONITOR_DEFAULTTONEAREST, OUT_DEFAULT_PRECIS, PAINTSTRUCT, PS_SOLID, SRCCOPY, TRANSPARENT,
};
use windows_sys::Win32::UI::Controls::MARGINS;
use windows_sys::Win32::UI::HiDpi::{GetDpiForMonitor, MDT_EFFECTIVE_DPI};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    ReleaseCapture, SetCapture, VK_DOWN, VK_ESCAPE, VK_LEFT, VK_LMENU, VK_LWIN, VK_MENU, VK_RETURN,
    VK_RIGHT, VK_RMENU, VK_RWIN, VK_SPACE, VK_UP,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, CreateWindowExW, DefWindowProcW, DestroyWindow, GetCursorPos, KillTimer,
    LoadCursorW, PostMessageW, RegisterClassExW, SetCursor, SetForegroundWindow, SetTimer,
    SetWindowsHookExW, ShowWindow, SystemParametersInfoW, UnhookWindowsHookEx, CS_HREDRAW,
    CS_VREDRAW, HHOOK, IDC_ARROW, MSLLHOOKSTRUCT, SPI_GETMENUSHOWDELAY, SW_SHOWNOACTIVATE,
    WH_MOUSE_LL, WM_APP, WM_CAPTURECHANGED, WM_COMMAND, WM_ERASEBKGND, WM_KEYDOWN, WM_LBUTTONDOWN,
    WM_LBUTTONUP, WM_MBUTTONDOWN, WM_MOUSEMOVE, WM_PAINT, WM_RBUTTONDOWN, WM_RBUTTONUP, WM_TIMER,
    WNDCLASSEXW, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_POPUP,
};

const MENU_CLASS_NAME: &str = "WinSpacesMenu";

/// Posted (never sent) so teardown always runs from the message loop, outside
/// any hook callback or state borrow.
const WM_MENU_CLOSE: u32 = WM_APP + 41;

const TIMER_OPEN_SUB: usize = 1;
const TIMER_CLOSE_SUB: usize = 2;

// Segoe Fluent Icons glyphs (same codepoints exist in Segoe MDL2 Assets).
pub const GLYPH_TASK_VIEW: u16 = 0xE7C4;
pub const GLYPH_MONITOR: u16 = 0xE7F4;
pub const GLYPH_CAMERA: u16 = 0xE722;
pub const GLYPH_RESTORE: u16 = 0xE777;
pub const GLYPH_SETTINGS: u16 = 0xE713;
pub const GLYPH_SYNC: u16 = 0xE895;
pub const GLYPH_REFRESH: u16 = 0xE72C;
pub const GLYPH_CLOSE: u16 = 0xE8BB;
const GLYPH_CHECK: u16 = 0xE73E;
const GLYPH_CHEVRON: u16 = 0xE76C;

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

// COLORREF (0x00BBGGRR) helper.
const fn rgb(r: u8, g: u8, b: u8) -> u32 {
    (r as u32) | ((g as u32) << 8) | ((b as u32) << 16)
}

/// Background opacity over the acrylic backdrop (255 = opaque fallback).
const BG_ALPHA: u32 = 232;

/// Flyout palette, matching the Windows 11 flyout look in both modes.
/// Resolved on every menu open from the shared theme preference
/// (`settings_ui::theme`: App Theme selector, falling back to the OS apps
/// mode), so the menu follows live theme changes without any hooks.
struct MenuTheme {
    text: u32,
    dim: u32,
    hover: u32,
    separator: u32,
    bg_r: u32,
    bg_g: u32,
    bg_b: u32,
}

fn menu_theme(light: bool) -> MenuTheme {
    if light {
        MenuTheme {
            text: rgb(0x1B, 0x1B, 0x1B),
            dim: rgb(0x60, 0x60, 0x60),
            hover: rgb(0xEA, 0xEA, 0xEA),
            separator: rgb(0xE0, 0xE0, 0xE0),
            bg_r: 0xF9,
            bg_g: 0xF9,
            bg_b: 0xF9,
        }
    } else {
        MenuTheme {
            text: rgb(0xF5, 0xF5, 0xF5),
            dim: rgb(0xA6, 0xA6, 0xA6),
            hover: rgb(0x3D, 0x3D, 0x3D),
            separator: rgb(0x45, 0x45, 0x45),
            bg_r: 0x2C,
            bg_g: 0x2C,
            bg_b: 0x2C,
        }
    }
}

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
    fn is_selectable(&self) -> bool {
        matches!(self, MenuEntry::Item(_))
    }
}

#[derive(Clone, Copy)]
struct Row {
    top: i32,
    height: i32,
}

struct MenuWindow {
    hwnd: HWND,
    /// Screen position of the window's top-left corner.
    x: i32,
    y: i32,
    width: i32,
    height: i32,
    entries: Vec<MenuEntry>,
    rows: Vec<Row>,
    hover: Option<usize>,
    sel: Option<usize>,
}

struct Fonts {
    text: HFONT,
    small: HFONT,
    glyph: HFONT,
    glyph_small: HFONT,
}

struct MenuState {
    owner: HWND,
    root: MenuWindow,
    sub: Option<MenuWindow>,
    sub_parent: Option<usize>,
    pending_sub: Option<usize>,
    fonts: Fonts,
    scale: f32,
    acrylic: bool,
    light: bool,
    theme: MenuTheme,
    /// Work area of the monitor the menu opened on, for submenu flipping.
    work: RECT,
    /// Low-level mouse hook alive only while the menu is open: `SetCapture`
    /// cannot be relied on (it needs the foreground thread), so this is the
    /// authoritative outside-click dismisser.
    mouse_hook: HHOOK,
}

thread_local! {
    static MENU_STATE: RefCell<Option<MenuState>> = const { RefCell::new(None) };
}

/// Windows build number via RtlGetVersion (GetVersionEx is shimmed by appcompat).
pub fn win_build() -> u32 {
    use std::sync::OnceLock;
    static BUILD: OnceLock<u32> = OnceLock::new();
    *BUILD.get_or_init(|| unsafe {
        type RtlGetVersionFn = unsafe extern "system" fn(
            *mut windows_sys::Win32::System::SystemInformation::OSVERSIONINFOW,
        ) -> i32;
        let ntdll = windows_sys::Win32::System::LibraryLoader::GetModuleHandleW(
            encode_wide("ntdll.dll").as_ptr(),
        );
        if !ntdll.is_null() {
            if let Some(proc_addr) = windows_sys::Win32::System::LibraryLoader::GetProcAddress(
                ntdll,
                c"RtlGetVersion".as_ptr() as _,
            ) {
                let rtl_get_version: RtlGetVersionFn = std::mem::transmute(proc_addr);
                let mut vi: windows_sys::Win32::System::SystemInformation::OSVERSIONINFOW =
                    std::mem::zeroed();
                vi.dwOSVersionInfoSize = std::mem::size_of_val(&vi) as u32;
                if rtl_get_version(&mut vi) == 0 {
                    return vi.dwBuildNumber;
                }
            }
        }
        0
    })
}

pub fn is_menu_open() -> bool {
    MENU_STATE.with(|s| s.try_borrow().map(|st| st.is_some()).unwrap_or(false))
}

fn root_hwnd() -> HWND {
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
                    .and_then(|b| b.as_ref().map(|state| hit_test(state, pt) == Hit::Outside))
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
        }
    }
}

pub fn show_menu(owner: HWND, entries: Vec<MenuEntry>, anchor: POINT) {
    unsafe {
        close_menu();

        let hmon = MonitorFromPoint(anchor, MONITOR_DEFAULTTONEAREST);
        let mut dpi_x: u32 = 96;
        let mut dpi_y: u32 = 96;
        GetDpiForMonitor(hmon, MDT_EFFECTIVE_DPI, &mut dpi_x, &mut dpi_y);
        let scale = (dpi_x.max(96) as f32) / 96.0;

        let mut mi: MONITORINFO = std::mem::zeroed();
        mi.cbSize = std::mem::size_of::<MONITORINFO>() as u32;
        GetMonitorInfoW(hmon, &mut mi);
        let work = mi.rcWork;

        let fonts = create_fonts(scale);
        let (rows, width, height) = layout_window(&entries, &fonts, scale);

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
        let light =
            crate::settings_ui::theme::resolves_light(crate::settings_ui::theme::load_pref());
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
                scale,
                acrylic,
                light,
                theme: menu_theme(light),
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

unsafe fn delete_fonts(fonts: &Fonts) {
    DeleteObject(fonts.text as _);
    DeleteObject(fonts.small as _);
    DeleteObject(fonts.glyph as _);
    DeleteObject(fonts.glyph_small as _);
}

unsafe fn create_font(face: &str, height: i32, weight: i32) -> HFONT {
    let name = encode_wide(face);
    CreateFontW(
        -height,
        0,
        0,
        0,
        weight,
        0,
        0,
        0,
        DEFAULT_CHARSET as u32,
        OUT_DEFAULT_PRECIS as u32,
        CLIP_DEFAULT_PRECIS as u32,
        CLEARTYPE_QUALITY as u32,
        DEFAULT_PITCH as u32,
        name.as_ptr(),
    )
}

unsafe fn create_fonts(scale: f32) -> Fonts {
    let px = |v: i32| (v as f32 * scale).round() as i32;
    Fonts {
        text: create_font("Segoe UI Variable Text", px(15), 400),
        small: create_font("Segoe UI Variable Text", px(12), 400),
        glyph: create_font("Segoe Fluent Icons", px(16), 400),
        glyph_small: create_font("Segoe Fluent Icons", px(12), 400),
    }
}

unsafe fn measure_text(hdc: HDC, font: HFONT, text: &str) -> i32 {
    SelectObject(hdc, font as _);
    let wide: Vec<u16> = text.encode_utf16().collect();
    let mut size = SIZE { cx: 0, cy: 0 };
    GetTextExtentPoint32W(hdc, wide.as_ptr(), wide.len() as i32, &mut size);
    size.cx
}

unsafe fn layout_window(entries: &[MenuEntry], fonts: &Fonts, scale: f32) -> (Vec<Row>, i32, i32) {
    let px = |v: i32| (v as f32 * scale).round() as i32;
    let hdc_screen = GetDC(null_mut());
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
    ReleaseDC(null_mut(), hdc_screen);

    (rows, max_w.min(px(MAX_W)), y + px(PAD_V))
}

unsafe fn ensure_class() {
    use std::sync::Once;
    static REGISTER: Once = Once::new();
    REGISTER.call_once(|| {
        let class_name = encode_wide(MENU_CLASS_NAME);
        let wc = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            style: CS_HREDRAW | CS_VREDRAW,
            lpfnWndProc: Some(menu_wnd_proc),
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
    });
}

unsafe fn create_menu_window(
    owner: HWND,
    x: i32,
    y: i32,
    width: i32,
    height: i32,
    acrylic: bool,
    light: bool,
) -> HWND {
    ensure_class();
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
        crate::get_app_instance(),
        null_mut(),
    );
    if hwnd.is_null() {
        return hwnd;
    }

    // Also steers the tint DWM applies to the acrylic backdrop.
    let dark: i32 = if light { 0 } else { 1 };
    DwmSetWindowAttribute(
        hwnd,
        DWMWA_USE_IMMERSIVE_DARK_MODE as _,
        &dark as *const _ as _,
        std::mem::size_of::<i32>() as u32,
    );
    // DWMWCP_ROUND: Win11 rounds the popup and supplies the shadow.
    let corner: u32 = 2;
    DwmSetWindowAttribute(
        hwnd,
        33, // DWMWA_WINDOW_CORNER_PREFERENCE
        &corner as *const _ as _,
        std::mem::size_of::<u32>() as u32,
    );
    if acrylic {
        // "Sheet of glass" frame extension is REQUIRED before the backdrop
        // attribute has any visible effect (PowerToys AltWindowCycle recipe).
        let margins = MARGINS {
            cxLeftWidth: -1,
            cxRightWidth: -1,
            cyTopHeight: -1,
            cyBottomHeight: -1,
        };
        DwmExtendFrameIntoClientArea(hwnd, &margins);
        let backdrop: i32 = 3; // DWMSBT_TRANSIENTWINDOW (acrylic)
        DwmSetWindowAttribute(
            hwnd,
            DWMWA_SYSTEMBACKDROP_TYPE as _,
            &backdrop as *const _ as _,
            std::mem::size_of::<i32>() as u32,
        );
    }
    hwnd
}

fn submenu_show_delay() -> u32 {
    let mut delay: u32 = 400;
    unsafe {
        SystemParametersInfoW(SPI_GETMENUSHOWDELAY, 0, &mut delay as *mut _ as _, 0);
    }
    delay.clamp(50, 1000)
}

#[derive(PartialEq, Clone, Copy)]
enum Hit {
    Root(usize),
    RootBlank,
    Sub(usize),
    SubBlank,
    Outside,
}

fn hit_test_window(win: &MenuWindow, pt: POINT) -> Option<Option<usize>> {
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

fn hit_test(state: &MenuState, pt: POINT) -> Hit {
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

unsafe fn cursor_pos() -> POINT {
    let mut pt = POINT { x: 0, y: 0 };
    GetCursorPos(&mut pt);
    pt
}

unsafe fn on_mouse_move() {
    let pt = cursor_pos();
    MENU_STATE.with(|s| {
        let mut borrow = s.borrow_mut();
        let Some(state) = borrow.as_mut() else {
            return;
        };
        let hit = hit_test(state, pt);

        let new_root_hover = if let Hit::Root(i) = hit {
            Some(i)
        } else {
            None
        };
        let new_sub_hover = if let Hit::Sub(i) = hit { Some(i) } else { None };

        if new_root_hover != state.root.hover {
            state.root.hover = new_root_hover;
            state.root.sel = None;
            InvalidateRect(state.root.hwnd, std::ptr::null(), 0);
        }
        if let Some(sub) = state.sub.as_mut() {
            if new_sub_hover != sub.hover {
                sub.hover = new_sub_hover;
                sub.sel = None;
                InvalidateRect(sub.hwnd, std::ptr::null(), 0);
            }
        }

        // Submenu open/close timers follow the system hover delay.
        match hit {
            Hit::Root(i) => {
                let has_sub =
                    matches!(&state.root.entries[i], MenuEntry::Item(it) if it.submenu.is_some());
                if has_sub {
                    if state.sub_parent == Some(i) {
                        KillTimer(state.root.hwnd, TIMER_CLOSE_SUB);
                    } else if state.pending_sub != Some(i) {
                        state.pending_sub = Some(i);
                        SetTimer(state.root.hwnd, TIMER_OPEN_SUB, submenu_show_delay(), None);
                    }
                } else {
                    state.pending_sub = None;
                    KillTimer(state.root.hwnd, TIMER_OPEN_SUB);
                    if state.sub.is_some() {
                        SetTimer(state.root.hwnd, TIMER_CLOSE_SUB, submenu_show_delay(), None);
                    }
                }
            }
            Hit::Sub(_) | Hit::SubBlank => {
                state.pending_sub = None;
                KillTimer(state.root.hwnd, TIMER_OPEN_SUB);
                KillTimer(state.root.hwnd, TIMER_CLOSE_SUB);
            }
            _ => {}
        }
    });
}

unsafe fn on_mouse_down() {
    let pt = cursor_pos();
    let outside = MENU_STATE.with(|s| {
        s.borrow()
            .as_ref()
            .map(|state| hit_test(state, pt) == Hit::Outside)
            .unwrap_or(false)
    });
    if outside {
        close_menu();
    }
}

unsafe fn on_mouse_up() {
    let pt = cursor_pos();
    let mut command: Option<(HWND, usize)> = None;
    MENU_STATE.with(|s| {
        let mut borrow = s.borrow_mut();
        let Some(state) = borrow.as_mut() else {
            return;
        };
        match hit_test(state, pt) {
            Hit::Root(i) => {
                let has_sub =
                    matches!(&state.root.entries[i], MenuEntry::Item(it) if it.submenu.is_some());
                if has_sub {
                    if state.sub_parent != Some(i) {
                        open_submenu(state, i, false);
                    }
                } else if let MenuEntry::Item(it) = &state.root.entries[i] {
                    command = Some((state.owner, it.id));
                }
            }
            Hit::Sub(i) => {
                if let Some(sub) = &state.sub {
                    if let MenuEntry::Item(it) = &sub.entries[i] {
                        command = Some((state.owner, it.id));
                    }
                }
            }
            _ => {}
        }
    });
    if let Some((owner, id)) = command {
        PostMessageW(owner, WM_COMMAND, id, 0);
        close_menu();
    }
}

unsafe fn open_submenu(state: &mut MenuState, parent_idx: usize, via_keyboard: bool) {
    if let Some(sub) = state.sub.take() {
        DestroyWindow(sub.hwnd);
        state.sub_parent = None;
    }
    let MenuEntry::Item(parent) = &state.root.entries[parent_idx] else {
        return;
    };
    let Some(sub_entries) = &parent.submenu else {
        return;
    };
    // Clone the submenu content into its own window state; submenus are one
    // level deep so the clone is a handful of small structs.
    let entries: Vec<MenuEntry> = sub_entries
        .iter()
        .map(|e| match e {
            MenuEntry::Header(t) => MenuEntry::Header(t.clone()),
            MenuEntry::Separator => MenuEntry::Separator,
            MenuEntry::Item(it) => MenuEntry::Item(MenuItemData {
                id: it.id,
                glyph: it.glyph,
                label: it.label.clone(),
                shortcut: it.shortcut.clone(),
                checked: it.checked,
                submenu: None,
            }),
        })
        .collect();

    let px = |v: i32| (v as f32 * state.scale).round() as i32;
    let (rows, width, height) = layout_window(&entries, &state.fonts, state.scale);

    let parent_row = state.root.rows[parent_idx];
    // Slight overlap like native menus; flip to the left edge when the
    // submenu would leave the work area.
    let mut x = state.root.x + state.root.width - px(4);
    if x + width > state.work.right {
        x = (state.root.x - width + px(4)).max(state.work.left);
    }
    let mut y = state.root.y + parent_row.top - px(PAD_V);
    if y + height > state.work.bottom {
        y = (state.work.bottom - height).max(state.work.top);
    }

    let hwnd = create_menu_window(
        state.root.hwnd,
        x,
        y,
        width,
        height,
        state.acrylic,
        state.light,
    );
    if hwnd.is_null() {
        return;
    }
    ShowWindow(hwnd, SW_SHOWNOACTIVATE);

    let sel = if via_keyboard {
        entries.iter().position(|e| e.is_selectable())
    } else {
        None
    };
    state.sub = Some(MenuWindow {
        hwnd,
        x,
        y,
        width,
        height,
        entries,
        rows,
        hover: None,
        sel,
    });
    state.sub_parent = Some(parent_idx);
    state.pending_sub = None;
}

unsafe fn close_submenu(state: &mut MenuState) {
    if let Some(sub) = state.sub.take() {
        DestroyWindow(sub.hwnd);
    }
    state.sub_parent = None;
}

unsafe fn on_timer(id: usize) {
    MENU_STATE.with(|s| {
        let mut borrow = s.borrow_mut();
        let Some(state) = borrow.as_mut() else {
            return;
        };
        KillTimer(state.root.hwnd, id);
        match id {
            TIMER_OPEN_SUB => {
                if let Some(pending) = state.pending_sub.take() {
                    // Only open if the cursor is still on that row.
                    if state.root.hover == Some(pending) {
                        open_submenu(state, pending, false);
                    }
                }
            }
            TIMER_CLOSE_SUB => {
                // Don't close while the cursor sits inside the submenu.
                let pt = cursor_pos();
                let in_sub = state
                    .sub
                    .as_ref()
                    .map(|sub| hit_test_window(sub, pt).is_some())
                    .unwrap_or(false);
                if !in_sub {
                    close_submenu(state);
                }
            }
            _ => {}
        }
    });
}

fn step_selection(entries: &[MenuEntry], current: Option<usize>, dir: i32) -> Option<usize> {
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

unsafe fn on_key(vk: u32) {
    let mut command: Option<(HWND, usize)> = None;
    let mut close_all = false;
    MENU_STATE.with(|s| {
        let mut borrow = s.borrow_mut();
        let Some(state) = borrow.as_mut() else {
            return;
        };
        match vk as u16 {
            VK_ESCAPE => {
                if state.sub.is_some() {
                    close_submenu(state);
                    InvalidateRect(state.root.hwnd, std::ptr::null(), 0);
                } else {
                    close_all = true;
                }
            }
            VK_UP | VK_DOWN => {
                let dir = if vk as u16 == VK_DOWN { 1 } else { -1 };
                let win = state.sub.as_mut().unwrap_or(&mut state.root);
                win.sel = step_selection(&win.entries, win.sel, dir);
                win.hover = None;
                InvalidateRect(win.hwnd, std::ptr::null(), 0);
            }
            VK_LEFT => {
                if state.sub.is_some() {
                    close_submenu(state);
                    InvalidateRect(state.root.hwnd, std::ptr::null(), 0);
                }
            }
            VK_RIGHT => {
                if state.sub.is_none() {
                    if let Some(sel) = state.root.sel {
                        let has_sub = matches!(&state.root.entries[sel], MenuEntry::Item(it) if it.submenu.is_some());
                        if has_sub {
                            open_submenu(state, sel, true);
                        }
                    }
                }
            }
            VK_RETURN | VK_SPACE => {
                enum Action {
                    OpenSub(usize),
                    Command(usize),
                }
                let mut action = None;
                {
                    let (win, is_sub) = match state.sub.as_ref() {
                        Some(sub) => (sub, true),
                        None => (&state.root, false),
                    };
                    if let Some(sel) = win.sel {
                        if let MenuEntry::Item(it) = &win.entries[sel] {
                            action = if !is_sub && it.submenu.is_some() {
                                Some(Action::OpenSub(sel))
                            } else {
                                Some(Action::Command(it.id))
                            };
                        }
                    }
                }
                match action {
                    Some(Action::OpenSub(idx)) => open_submenu(state, idx, true),
                    Some(Action::Command(id)) => command = Some((state.owner, id)),
                    None => {}
                }
            }
            _ => {}
        }
    });
    if let Some((owner, id)) = command {
        PostMessageW(owner, WM_COMMAND, id, 0);
        close_all = true;
    }
    if close_all {
        close_menu();
    }
}

unsafe fn draw_text(hdc: HDC, text: &str, rect: &mut RECT, flags: u32) {
    let wide: Vec<u16> = text.encode_utf16().collect();
    DrawTextW(
        hdc,
        wide.as_ptr(),
        wide.len() as i32,
        rect,
        flags | DT_NOPREFIX,
    );
}

unsafe fn draw_glyph(hdc: HDC, glyph: u16, rect: &mut RECT, flags: u32) {
    let wide = [glyph];
    DrawTextW(hdc, wide.as_ptr(), 1, rect, flags | DT_NOPREFIX);
}

/// Render one menu window into a 32bpp premultiplied DIB and blit it, alpha
/// channel included, onto the window surface. Background pixels carry
/// `BG_ALPHA` so the DWM acrylic shows through; everything GDI touched
/// (text, highlights, separators — GDI zeroes alpha) is fixed up to opaque.
unsafe fn render_window(win: &MenuWindow, state: &MenuState, target: HDC) {
    let w = win.width;
    let h = win.height;
    let px = |v: i32| (v as f32 * state.scale).round() as i32;

    let hdc_mem = CreateCompatibleDC(target);
    let mut bmi: BITMAPINFO = std::mem::zeroed();
    bmi.bmiHeader.biSize = std::mem::size_of::<BITMAPINFOHEADER>() as u32;
    bmi.bmiHeader.biWidth = w;
    bmi.bmiHeader.biHeight = -h; // top-down
    bmi.bmiHeader.biPlanes = 1;
    bmi.bmiHeader.biBitCount = 32;
    bmi.bmiHeader.biCompression = BI_RGB;
    let mut bits: *mut u8 = null_mut();
    let dib = CreateDIBSection(
        target,
        &bmi,
        DIB_RGB_COLORS,
        &mut bits as *mut *mut u8 as _,
        null_mut(),
        0,
    );
    if dib.is_null() {
        DeleteDC(hdc_mem);
        return;
    }
    let old_bm = SelectObject(hdc_mem, dib as _);

    // 1. Premultiplied background tint (opaque when acrylic is unavailable).
    let theme = &state.theme;
    let alpha = if state.acrylic { BG_ALPHA } else { 255 };
    let pm = |c: u32| c * alpha / 255;
    let bg_pixel = (alpha << 24) | (pm(theme.bg_r) << 16) | (pm(theme.bg_g) << 8) | pm(theme.bg_b);
    let pixels = std::slice::from_raw_parts_mut(bits as *mut u32, (w * h) as usize);
    pixels.fill(bg_pixel);

    // 2. Foreground via GDI.
    SetBkMode(hdc_mem, TRANSPARENT as i32);
    let hover_brush = CreateSolidBrush(theme.hover);
    let hover_pen = CreatePen(PS_SOLID, 1, theme.hover);
    let sep_brush = CreateSolidBrush(theme.separator);

    let any_chevron = win
        .entries
        .iter()
        .any(|e| matches!(e, MenuEntry::Item(it) if it.submenu.is_some()));

    for (idx, entry) in win.entries.iter().enumerate() {
        let row = win.rows[idx];
        match entry {
            MenuEntry::Header(text) => {
                SelectObject(hdc_mem, state.fonts.small as _);
                SetTextColor(hdc_mem, theme.dim);
                let mut r = RECT {
                    left: px(ICON_X),
                    top: row.top,
                    right: w - px(RIGHT_PAD),
                    bottom: row.top + row.height,
                };
                draw_text(
                    hdc_mem,
                    text,
                    &mut r,
                    DT_SINGLELINE | DT_VCENTER | DT_END_ELLIPSIS,
                );
            }
            MenuEntry::Separator => {
                let line_y = row.top + row.height / 2;
                let r = RECT {
                    left: px(12),
                    top: line_y,
                    right: w - px(12),
                    bottom: line_y + px(1).max(1),
                };
                FillRect(hdc_mem, &r, sep_brush as _);
            }
            MenuEntry::Item(it) => {
                let highlighted = win.hover == Some(idx) || win.sel == Some(idx);
                if highlighted {
                    SelectObject(hdc_mem, hover_brush as _);
                    SelectObject(hdc_mem, hover_pen as _);
                    RoundRect(
                        hdc_mem,
                        px(HOVER_INSET),
                        row.top + px(2),
                        w - px(HOVER_INSET),
                        row.top + row.height - px(2),
                        px(HOVER_RADIUS),
                        px(HOVER_RADIUS),
                    );
                }

                // Icon column: checkmark wins over the item glyph.
                let icon_glyph = if it.checked {
                    Some(GLYPH_CHECK)
                } else {
                    it.glyph
                };
                if let Some(g) = icon_glyph {
                    SelectObject(hdc_mem, state.fonts.glyph as _);
                    SetTextColor(hdc_mem, theme.text);
                    let mut r = RECT {
                        left: px(ICON_X),
                        top: row.top,
                        right: px(TEXT_X) - px(6),
                        bottom: row.top + row.height,
                    };
                    draw_glyph(hdc_mem, g, &mut r, DT_SINGLELINE | DT_VCENTER | DT_CENTER);
                }

                let chevron_space = if any_chevron { px(CHEVRON_W) } else { 0 };
                let right_edge = w - px(RIGHT_PAD) - chevron_space;

                // Label.
                SelectObject(hdc_mem, state.fonts.text as _);
                SetTextColor(hdc_mem, theme.text);
                let mut r = RECT {
                    left: px(TEXT_X),
                    top: row.top,
                    right: right_edge,
                    bottom: row.top + row.height,
                };
                draw_text(
                    hdc_mem,
                    &it.label,
                    &mut r,
                    DT_SINGLELINE | DT_VCENTER | DT_END_ELLIPSIS,
                );

                // Shortcut / secondary text, right-aligned and dimmed.
                if let Some(shortcut) = &it.shortcut {
                    SelectObject(hdc_mem, state.fonts.small as _);
                    SetTextColor(hdc_mem, theme.dim);
                    let mut r = RECT {
                        left: px(TEXT_X),
                        top: row.top,
                        right: right_edge,
                        bottom: row.top + row.height,
                    };
                    draw_text(
                        hdc_mem,
                        shortcut,
                        &mut r,
                        DT_SINGLELINE | DT_VCENTER | DT_RIGHT,
                    );
                }

                if it.submenu.is_some() {
                    SelectObject(hdc_mem, state.fonts.glyph_small as _);
                    SetTextColor(hdc_mem, theme.dim);
                    let mut r = RECT {
                        left: w - px(RIGHT_PAD) - px(16),
                        top: row.top,
                        right: w - px(RIGHT_PAD) + px(4),
                        bottom: row.top + row.height,
                    };
                    draw_glyph(
                        hdc_mem,
                        GLYPH_CHEVRON,
                        &mut r,
                        DT_SINGLELINE | DT_VCENTER | DT_CENTER,
                    );
                }
            }
        }
    }

    DeleteObject(hover_brush as _);
    DeleteObject(hover_pen as _);
    DeleteObject(sep_brush as _);

    // 3. Alpha fixup: GDI wrote alpha=0 on every pixel it touched; promote
    // those to opaque so text and highlights sit solid on the acrylic.
    for p in pixels.iter_mut() {
        if *p >> 24 == 0 {
            *p |= 0xFF00_0000;
        }
    }

    // 4. Raw copy (BitBlt preserves the alpha channel) onto the window.
    BitBlt(target, 0, 0, w, h, hdc_mem, 0, 0, SRCCOPY);

    SelectObject(hdc_mem, old_bm);
    DeleteObject(dib as _);
    DeleteDC(hdc_mem);
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
            render_window(&state.root, state, hdc);
        } else if let Some(sub) = &state.sub {
            if hwnd == sub.hwnd {
                render_window(sub, state, hdc);
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
            on_mouse_move();
            0
        }
        WM_LBUTTONDOWN | WM_RBUTTONDOWN => {
            on_mouse_down();
            0
        }
        WM_LBUTTONUP | WM_RBUTTONUP => {
            on_mouse_up();
            0
        }
        WM_KEYDOWN => {
            on_key(wparam as u32);
            0
        }
        WM_TIMER => {
            on_timer(wparam);
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
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}
