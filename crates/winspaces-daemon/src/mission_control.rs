use crate::desktop::is_valid_window;
use crate::log_info;
use std::cell::RefCell;
use std::collections::HashMap;
use std::ptr::null_mut;
use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, SIZE, WPARAM};
use windows_sys::Win32::Graphics::Dwm::{
    DwmQueryThumbnailSourceSize, DwmRegisterThumbnail, DwmSetWindowAttribute,
    DwmUnregisterThumbnail, DwmUpdateThumbnailProperties, DWMWA_SYSTEMBACKDROP_TYPE,
    DWMWA_USE_IMMERSIVE_DARK_MODE, DWM_THUMBNAIL_PROPERTIES, DWM_TNP_OPACITY,
    DWM_TNP_RECTDESTINATION, DWM_TNP_SOURCECLIENTAREAONLY, DWM_TNP_VISIBLE,
};
use windows_sys::Win32::Graphics::Gdi::{
    BeginPaint, BitBlt, CreateCompatibleBitmap, CreateCompatibleDC, CreateFontW, CreatePen,
    CreateSolidBrush, DeleteDC, DeleteObject, DrawTextW, EndPaint, FillRect, GetDC, GetDeviceCaps,
    GetMonitorInfoW, InvalidateRect, ReleaseDC, RoundRect, ScreenToClient, SelectObject, SetBkMode,
    SetTextColor, SetViewportOrgEx, DT_CENTER, DT_END_ELLIPSIS, DT_SINGLELINE, DT_VCENTER, HFONT,
    MONITORINFO, PAINTSTRUCT, PS_DASH, PS_SOLID, SRCCOPY, TRANSPARENT, VREFRESH,
};
use windows_sys::Win32::System::SystemInformation::GetTickCount;
use windows_sys::Win32::UI::HiDpi::GetDpiForWindow;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    GetKeyState, ReleaseCapture, SetCapture, VK_CONTROL, VK_ESCAPE, VK_LEFT, VK_NUMPAD1,
    VK_NUMPAD9, VK_RIGHT, VK_SHIFT,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DrawIconEx, GetClientRect, GetCursorPos, GetSystemMetrics,
    GetWindowTextW, KillTimer, RegisterClassExW, SetForegroundWindow, SetTimer, SetWindowPos,
    ShowWindow, CS_HREDRAW, CS_VREDRAW, DI_NORMAL, GCLP_HICON, GCLP_HICONSM, ICON_BIG, ICON_SMALL,
    ICON_SMALL2, SM_CXDRAG, SM_CYDRAG, SWP_FRAMECHANGED, SWP_NOACTIVATE, SWP_NOZORDER, SW_HIDE,
    SW_SHOW, WM_CAPTURECHANGED, WM_ERASEBKGND, WM_GETICON, WM_KEYDOWN, WM_LBUTTONDOWN,
    WM_LBUTTONUP, WM_MOUSEMOVE, WM_PAINT, WM_TIMER, WNDCLASSEXW, WS_EX_TOOLWINDOW, WS_EX_TOPMOST,
    WS_POPUP,
};
use winspaces_common::MAX_DESKTOPS;

#[allow(clippy::upper_case_acronyms)]
pub type HICON = *mut std::ffi::c_void;

const fn rgb(r: u8, g: u8, b: u8) -> u32 {
    (r as u32) | ((g as u32) << 8) | ((b as u32) << 16)
}

const MC_CLASS_NAME: &str = "WinSpacesMissionControl";

/// One-shot timer that flushes the final frame of a throttled space drag.
const TIMER_DRAG_PAINT: usize = 1;

#[derive(Clone)]
pub struct SpaceCard {
    pub desk_idx: usize,
    pub rect: RECT,
    pub window_count: usize,
    pub is_active: bool,
}

#[derive(Clone)]
pub struct WindowCard {
    pub hwnd: HWND,
    pub h_thumb: isize,
    pub h_icon: HICON,
    pub card_rect: RECT,
    pub thumb_rect: RECT,
    pub title: String,
}

pub struct MissionControl {
    pub hwnd: HWND,
    pub is_visible: bool,
    pub active_mon_idx: usize,
    pub active_desk_idx: usize,
    pub scale: f32,
    pub space_cards: Vec<SpaceCard>,
    pub window_cards: Vec<WindowCard>,
    /// The "+" tile sits *outside* `space_cards` on purpose: every consumer of
    /// that vector (click-switch, drag-drop, digit render) may then assume it
    /// contains real spaces only.
    pub plus_rect: RECT,
    pub plus_visible: bool,
    pub hovered_plus: bool,
    pub hovered_space: Option<usize>,
    /// Space card whose close button the pointer is over. Distinct from
    /// `hovered_space`: the button sits inside the card, and clicking it must
    /// remove the space rather than switch to it.
    pub hovered_close: Option<usize>,
    pub hovered_window: Option<usize>,
    pub dragging_window: Option<usize>,
    /// True once the pressed pointer travels past the system drag threshold;
    /// separates a click-to-focus from a real drag so the ghost never
    /// flickers on a plain click.
    pub drag_active: bool,
    pub drag_offset: POINT,
    pub dragging_space: Option<usize>,
    pub drag_space_active: bool,
    pub drag_space_current_x: i32,
    pub drag_space_target_slot: Option<usize>,
    /// Tick of the last space-drag repaint, and the display's frame interval.
    /// A mouse reports far faster than the panel refreshes, and this window is
    /// DWM-composited: blitting more than once per frame lets the compositor
    /// sample a half-updated surface, which reads as tearing across the card.
    pub drag_paint_tick: u32,
    pub drag_frame_ms: u32,
    pub h_font_title: HFONT,
    pub h_font_card: HFONT,
    pub h_font_small: HFONT,
    /// The ✕ in the close button. Its own size because the button is a fixed
    /// 20px circle — it cannot follow `h_font_small` when that grows.
    pub h_font_close: HFONT,
    /// Segoe Fluent Icons, for the "new space" tile's glyph.
    pub h_font_glyph: HFONT,
}

thread_local! {
    static MC_STATE: RefCell<MissionControl> = const { RefCell::new(MissionControl::new()) };
}

impl MissionControl {
    pub const fn new() -> Self {
        Self {
            hwnd: null_mut(),
            is_visible: false,
            active_mon_idx: 0,
            active_desk_idx: 0,
            scale: 1.0,
            space_cards: Vec::new(),
            window_cards: Vec::new(),
            plus_rect: RECT {
                left: 0,
                top: 0,
                right: 0,
                bottom: 0,
            },
            plus_visible: false,
            hovered_plus: false,
            hovered_space: None,
            hovered_close: None,
            hovered_window: None,
            dragging_window: None,
            drag_active: false,
            drag_offset: POINT { x: 0, y: 0 },
            dragging_space: None,
            drag_space_active: false,
            drag_space_current_x: 0,
            drag_space_target_slot: None,
            drag_paint_tick: 0,
            drag_frame_ms: 16,
            h_font_title: null_mut(),
            h_font_card: null_mut(),
            h_font_small: null_mut(),
            h_font_close: null_mut(),
            h_font_glyph: null_mut(),
        }
    }
}

pub fn init_mission_control() {
    log_info!("Initialized Mission Control subsystem");
}

pub fn is_mission_control_active() -> bool {
    MC_STATE.with(|s| s.borrow().is_visible)
}

pub fn toggle_mission_control(app_state: &mut crate::AppState) {
    if is_mission_control_active() {
        hide_mission_control();
    } else {
        show_mission_control(app_state);
    }
}

/// `SendMessageW(WM_GETICON)` would block the daemon indefinitely on a hung
/// target; use a short abort-if-hung timeout instead.
unsafe fn send_geticon_timeout(hwnd: HWND, icon_kind: u32) -> HICON {
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        SendMessageTimeoutW, SMTO_ABORTIFHUNG, SMTO_BLOCK,
    };
    let mut result: usize = 0;
    let ok = SendMessageTimeoutW(
        hwnd,
        WM_GETICON,
        icon_kind as _,
        0,
        SMTO_ABORTIFHUNG | SMTO_BLOCK,
        100,
        &mut result,
    );
    if ok != 0 {
        result as HICON
    } else {
        null_mut()
    }
}

unsafe fn get_window_icon(hwnd: HWND) -> HICON {
    let mut hicon = send_geticon_timeout(hwnd, ICON_SMALL2);
    if hicon.is_null() {
        hicon = send_geticon_timeout(hwnd, ICON_SMALL);
    }
    if hicon.is_null() {
        hicon = send_geticon_timeout(hwnd, ICON_BIG);
    }
    if hicon.is_null() {
        #[cfg(target_pointer_width = "64")]
        {
            hicon =
                windows_sys::Win32::UI::WindowsAndMessaging::GetClassLongPtrW(hwnd, GCLP_HICONSM)
                    as HICON;
        }
        #[cfg(not(target_pointer_width = "64"))]
        {
            hicon = windows_sys::Win32::UI::WindowsAndMessaging::GetClassLongW(hwnd, GCLP_HICONSM)
                as HICON;
        }
    }
    if hicon.is_null() {
        #[cfg(target_pointer_width = "64")]
        {
            hicon = windows_sys::Win32::UI::WindowsAndMessaging::GetClassLongPtrW(hwnd, GCLP_HICON)
                as HICON;
        }
        #[cfg(not(target_pointer_width = "64"))]
        {
            hicon = windows_sys::Win32::UI::WindowsAndMessaging::GetClassLongW(hwnd, GCLP_HICON)
                as HICON;
        }
    }
    hicon
}

unsafe fn update_fonts_for_dpi(mc: &mut MissionControl, dpi: u32) {
    if !mc.h_font_title.is_null() {
        DeleteObject(mc.h_font_title);
        DeleteObject(mc.h_font_card);
        DeleteObject(mc.h_font_small);
        DeleteObject(mc.h_font_close);
        DeleteObject(mc.h_font_glyph);
    }

    let scale = (dpi as f32 / 96.0).max(1.0);
    mc.scale = scale;

    let px = |pt: i32| (pt as f32 * scale).round() as i32;

    mc.h_font_title = create_segoe_font(px(20), 700); // Spaces Card Title
    mc.h_font_card = create_segoe_font(px(15), 600); // Window Card Title
    mc.h_font_small = create_segoe_font(px(18), 500); // Card subtitle + tile label
    mc.h_font_close = create_segoe_font(px(13), 500); // ✕ in the close button
                                                      // Same icon family the tray menu draws with, so the "add" affordance is
                                                      // the same mark in both surfaces.
    mc.h_font_glyph = create_font("Segoe Fluent Icons", px(24), 400);
}

pub fn show_mission_control(app_state: &mut crate::AppState) {
    app_state.desktop_mgr.scan_untracked_windows();
    unsafe {
        // 1. Determine active monitor & active desktop
        let mon_idx = app_state.desktop_mgr.get_active_monitor_index();
        if mon_idx >= app_state.desktop_mgr.monitors.len() {
            return;
        }
        let desk_idx = app_state.desktop_mgr.monitors[mon_idx].current;
        let hmon = app_state.desktop_mgr.monitors[mon_idx].hmon;

        let mut mi: MONITORINFO = std::mem::zeroed();
        mi.cbSize = std::mem::size_of::<MONITORINFO>() as u32;
        if GetMonitorInfoW(hmon, &mut mi) == 0 {
            return;
        }

        let mon_rect = mi.rcMonitor;
        let width = mon_rect.right - mon_rect.left;
        let height = mon_rect.bottom - mon_rect.top;

        MC_STATE.with(|s| {
            let mut mc = s.borrow_mut();
            if mc.is_visible {
                return;
            }

            mc.active_mon_idx = mon_idx;
            mc.active_desk_idx = desk_idx;
            mc.space_cards.clear();
            mc.window_cards.clear();
            mc.hovered_space = None;
            mc.hovered_window = None;
            mc.dragging_window = None;
            mc.drag_active = false;
            mc.dragging_space = None;
            mc.drag_space_active = false;
            mc.drag_space_current_x = 0;
            mc.drag_space_target_slot = None;

            // 2. Ensure Window Class & HWND
            if mc.hwnd.is_null() {
                let hinst = crate::get_app_instance();
                let class_name = crate::tray::encode_wide(MC_CLASS_NAME);
                let wc = WNDCLASSEXW {
                    cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
                    style: CS_HREDRAW | CS_VREDRAW,
                    lpfnWndProc: Some(mc_wnd_proc),
                    cbClsExtra: 0,
                    cbWndExtra: 0,
                    hInstance: hinst,
                    hIcon: null_mut(),
                    // A null class cursor keeps whatever cursor was active
                    // when the overlay opened (often the busy spinner).
                    hCursor: windows_sys::Win32::UI::WindowsAndMessaging::LoadCursorW(
                        null_mut(),
                        windows_sys::Win32::UI::WindowsAndMessaging::IDC_ARROW,
                    ),
                    hbrBackground: null_mut(),
                    lpszMenuName: std::ptr::null(),
                    lpszClassName: class_name.as_ptr(),
                    hIconSm: null_mut(),
                };
                RegisterClassExW(&wc);

                mc.hwnd = CreateWindowExW(
                    WS_EX_TOPMOST | WS_EX_TOOLWINDOW,
                    class_name.as_ptr(),
                    std::ptr::null(),
                    WS_POPUP,
                    mon_rect.left,
                    mon_rect.top,
                    width,
                    height,
                    null_mut(),
                    null_mut(),
                    hinst,
                    null_mut(),
                );

                if mc.hwnd.is_null() {
                    log_info!("Failed to create Mission Control overlay window");
                    return;
                }

                let dark_mode: i32 = 1;
                DwmSetWindowAttribute(
                    mc.hwnd,
                    DWMWA_USE_IMMERSIVE_DARK_MODE as _,
                    &dark_mode as *const _ as _,
                    std::mem::size_of::<i32>() as u32,
                );

                let backdrop: i32 = 3; // DWMSBT_ACRYLIC
                DwmSetWindowAttribute(
                    mc.hwnd,
                    DWMWA_SYSTEMBACKDROP_TYPE as _,
                    &backdrop as *const _ as _,
                    std::mem::size_of::<i32>() as u32,
                );
            } else {
                SetWindowPos(
                    mc.hwnd,
                    null_mut(),
                    mon_rect.left,
                    mon_rect.top,
                    width,
                    height,
                    SWP_NOZORDER | SWP_NOACTIVATE | SWP_FRAMECHANGED,
                );
            }

            // 3. Compute DPI-scaled metrics
            let dpi = GetDpiForWindow(mc.hwnd);
            let dpi = if dpi == 0 { 96 } else { dpi };
            update_fonts_for_dpi(&mut mc, dpi);

            // 4/5. Build spaces bar + window grid + thumbnails
            rebuild_cards(&mut mc, app_state, mon_idx, desk_idx, width, height);

            mc.is_visible = true;
            ShowWindow(mc.hwnd, SW_SHOW);
            SetForegroundWindow(mc.hwnd);
            InvalidateRect(mc.hwnd, std::ptr::null(), 1);
            log_info!(
                "Mission Control shown on Mon {} (Space {}) with {} window thumbnails",
                mon_idx + 1,
                desk_idx + 1,
                mc.window_cards.len()
            );
        });
    }
}

/// Rebuild the spaces bar and window-card grid (unregistering any existing
/// DWM thumbnails first). Shared by `show_mission_control` and
/// `refresh_mission_control`; assumes the overlay window and fonts exist.
unsafe fn rebuild_cards(
    mc: &mut MissionControl,
    app_state: &mut crate::AppState,
    mon_idx: usize,
    desk_idx: usize,
    width: i32,
    height: i32,
) {
    // Keep existing registrations keyed by source window: a kept thumbnail
    // never leaves DWM composition, so a refresh glides cards to their new
    // rects instead of blinking them out and back in. Whatever is left over
    // after the grid is rebuilt belongs to windows no longer on this space
    // and is unregistered at the end.
    let mut kept_thumbs: HashMap<HWND, isize> = HashMap::new();
    for card in &mc.window_cards {
        if card.h_thumb != 0 {
            kept_thumbs.insert(card.hwnd, card.h_thumb);
        }
    }
    mc.window_cards.clear();
    mc.space_cards.clear();

    let scale = mc.scale;
    let px = |val: i32| (val as f32 * scale).round() as i32;

    // Build Spaces Bar Layout (Top)
    let spaces_count = app_state.desktop_mgr.monitors[mon_idx].desktops.len();
    let has_plus = spaces_count < MAX_DESKTOPS;
    let bar = spaces_bar_metrics(spaces_count, has_plus, width, scale);
    let (card_w, card_h, gap, start_x, top_y) =
        (bar.card_w, bar.card_h, bar.gap, bar.start_x, bar.top_y);
    mc.plus_visible = has_plus;
    mc.plus_rect = bar.plus_rect;

    for d_idx in 0..spaces_count {
        let x = start_x + (d_idx as i32 * (card_w + gap));
        let card_rect = RECT {
            left: x,
            top: top_y,
            right: x + card_w,
            bottom: top_y + card_h,
        };
        let count = app_state.desktop_mgr.monitors[mon_idx].desktops[d_idx].len();
        mc.space_cards.push(SpaceCard {
            desk_idx: d_idx,
            rect: card_rect,
            window_count: count,
            is_active: d_idx == desk_idx,
        });
    }

    // 5. Build Exposé Window Grid Layout & Register DWM Live Thumbnails
    let visible_hwnds = app_state.desktop_mgr.monitors[mon_idx].desktops[desk_idx].clone();
    let valid_hwnds: Vec<HWND> = visible_hwnds
        .into_iter()
        .filter(|&h| is_valid_window(h))
        .collect();

    let grid_top = top_y + card_h + px(36);
    let grid_bottom = height - px(40);
    let grid_left = px(60);
    let grid_right = width - px(60);
    let grid_w = grid_right - grid_left;
    let grid_h = grid_bottom - grid_top;

    let num_wins = valid_hwnds.len();
    if num_wins > 0 {
        let (cols, rows) = match num_wins {
            1 => (1, 1),
            2 => (2, 1),
            3 => (3, 1),
            4 => (2, 2),
            5..=6 => (3, 2),
            7..=8 => (4, 2),
            9..=12 => (4, 3),
            13..=16 => (4, 4),
            _ => (5, ((num_wins as i32 + 4) / 5).max(1)),
        };

        let win_slot_w = (grid_w - (cols - 1) * px(24)) / cols;
        let win_slot_h = (grid_h - (rows - 1) * px(24)) / rows;
        let header_h = px(38);
        let thumb_margin = px(8);
        let card_min_w = px(180);
        let max_thumb_w = (win_slot_w - 2 * thumb_margin).max(px(100));
        let max_thumb_h = (win_slot_h - header_h - 2 * thumb_margin).max(px(100));

        for (idx, &target_hwnd) in valid_hwnds.iter().enumerate() {
            let r = (idx as i32) / cols;
            let c = (idx as i32) % cols;

            let items_in_row = if r == rows - 1 {
                num_wins as i32 - r * cols
            } else {
                cols
            };
            let row_offset_x = ((cols - items_in_row) * (win_slot_w + px(24))) / 2;

            let slot_left = grid_left + row_offset_x + c * (win_slot_w + px(24));
            let slot_top = grid_top + r * (win_slot_h + px(24));

            // Reuse the live registration when one exists, else register.
            let mut h_thumb: isize = kept_thumbs.remove(&target_hwnd).unwrap_or(0);
            let reused = h_thumb != 0;
            let hr = if reused {
                0
            } else {
                DwmRegisterThumbnail(mc.hwnd, target_hwnd, &mut h_thumb)
            };

            let (src_w, src_h) = if hr == 0 && h_thumb != 0 {
                let mut src_size: SIZE = std::mem::zeroed();
                let hr_size = DwmQueryThumbnailSourceSize(h_thumb, &mut src_size);
                if hr_size == 0 && src_size.cx > 0 && src_size.cy > 0 {
                    (src_size.cx as f32, src_size.cy as f32)
                } else {
                    let mut wr: RECT = std::mem::zeroed();
                    windows_sys::Win32::UI::WindowsAndMessaging::GetWindowRect(
                        target_hwnd,
                        &mut wr,
                    );
                    let w = (wr.right - wr.left).max(1);
                    let h = (wr.bottom - wr.top).max(1);
                    (w as f32, h as f32)
                }
            } else {
                let mut wr: RECT = std::mem::zeroed();
                windows_sys::Win32::UI::WindowsAndMessaging::GetWindowRect(target_hwnd, &mut wr);
                let w = (wr.right - wr.left).max(1);
                let h = (wr.bottom - wr.top).max(1);
                (w as f32, h as f32)
            };

            let src_aspect = (src_w / src_h).max(0.1);
            let max_aspect = max_thumb_w as f32 / max_thumb_h as f32;

            let (thumb_w, thumb_h) = if src_aspect > max_aspect {
                let tw = max_thumb_w;
                let th = ((max_thumb_w as f32 / src_aspect).round() as i32).max(px(40));
                (tw, th)
            } else {
                let th = max_thumb_h;
                let tw = ((max_thumb_h as f32 * src_aspect).round() as i32).max(px(40));
                (tw, th)
            };

            let card_w = (thumb_w + 2 * thumb_margin).max(card_min_w);
            let card_h = thumb_h + header_h + thumb_margin;

            let card_left = slot_left + (win_slot_w - card_w) / 2;
            let card_top = slot_top + (win_slot_h - card_h) / 2;
            let card_rect = RECT {
                left: card_left,
                top: card_top,
                right: card_left + card_w,
                bottom: card_top + card_h,
            };

            let thumb_left = card_left + (card_w - thumb_w) / 2;
            let thumb_top = card_top + header_h;
            let thumb_rect = RECT {
                left: thumb_left,
                top: thumb_top,
                right: thumb_left + thumb_w,
                bottom: thumb_top + thumb_h,
            };

            if hr == 0 && h_thumb != 0 {
                let mut props: DWM_THUMBNAIL_PROPERTIES = std::mem::zeroed();
                props.dwFlags = DWM_TNP_RECTDESTINATION
                    | DWM_TNP_VISIBLE
                    | DWM_TNP_OPACITY
                    | DWM_TNP_SOURCECLIENTAREAONLY;
                props.rcDestination = thumb_rect;
                props.fVisible = 1;
                props.opacity = 255;
                props.fSourceClientAreaOnly = 0;
                if DwmUpdateThumbnailProperties(h_thumb, &props) != 0 && reused {
                    // The kept handle went stale; demote to a fresh
                    // registration.
                    DwmUnregisterThumbnail(h_thumb);
                    h_thumb = 0;
                    if DwmRegisterThumbnail(mc.hwnd, target_hwnd, &mut h_thumb) == 0 && h_thumb != 0
                    {
                        DwmUpdateThumbnailProperties(h_thumb, &props);
                    }
                }
            }

            let mut title_buf = [0u16; 256];
            let len = GetWindowTextW(target_hwnd, title_buf.as_mut_ptr(), 256);
            let title = if len > 0 {
                String::from_utf16_lossy(&title_buf[..len as usize])
            } else {
                "Application Window".to_string()
            };

            let h_icon = get_window_icon(target_hwnd);

            mc.window_cards.push(WindowCard {
                hwnd: target_hwnd,
                h_thumb,
                h_icon,
                card_rect,
                thumb_rect,
                title,
            });
        }
    }

    // Windows no longer on this space keep no registration behind.
    for (_, h_thumb) in kept_thumbs {
        DwmUnregisterThumbnail(h_thumb);
    }
}

/// Re-sync an already-visible overlay with the desktop state in place —
/// no hide/show, so switching spaces from inside Mission Control (space-card
/// click, digit keys, global hotkeys) never flashes the overlay.
pub fn refresh_mission_control(app_state: &mut crate::AppState) {
    unsafe {
        MC_STATE.with(|s| {
            let mut mc = s.borrow_mut();
            if !mc.is_visible || mc.hwnd.is_null() {
                return;
            }
            let mon_idx = mc.active_mon_idx;
            if mon_idx >= app_state.desktop_mgr.monitors.len() {
                return;
            }
            let desk_idx = app_state.desktop_mgr.monitors[mon_idx].current;
            mc.active_desk_idx = desk_idx;

            let mut client_rect: RECT = std::mem::zeroed();
            GetClientRect(mc.hwnd, &mut client_rect);
            let width = client_rect.right - client_rect.left;
            let height = client_rect.bottom - client_rect.top;

            rebuild_cards(&mut mc, app_state, mon_idx, desk_idx, width, height);

            // Card indexes changed; stale hover/drag state must not survive.
            mc.dragging_window = None;
            mc.drag_active = false;
            mc.dragging_space = None;
            mc.drag_space_active = false;
            mc.drag_space_current_x = 0;
            mc.drag_space_target_slot = None;
            // Hover is re-derived rather than cleared: the pointer has not
            // moved, so leaving it blank would drop the highlight off the card
            // under the cursor until the user jiggles the mouse.
            resync_hover(&mut mc);

            // A switch may have activated another window; take the keyboard
            // back so Esc and the digit keys keep working.
            SetForegroundWindow(mc.hwnd);
            InvalidateRect(mc.hwnd, std::ptr::null(), 1);
        });
    }
}

/// Re-run every hover hit test for a client-space point. Window-card hover
/// freezes while a window drag is live: the ghost sweeping the grid would
/// otherwise flip the highlight (and repaint) on every card it crosses.
fn hover_at(mc: &mut MissionControl, pt: POINT) {
    let scale = mc.scale;
    mc.hovered_space = mc.space_cards.iter().position(|c| pt_in_rect(&c.rect, pt));
    mc.hovered_plus = mc.plus_visible && pt_in_rect(&mc.plus_rect, pt);
    mc.hovered_close = if mc.space_cards.len() > 1 {
        mc.space_cards
            .iter()
            .position(|c| pt_in_rect(&close_button_rect(&c.rect, scale), pt))
    } else {
        None
    };
    if !mc.drag_active {
        mc.hovered_window = mc
            .window_cards
            .iter()
            .position(|c| pt_in_rect(&c.card_rect, pt));
    }
}

/// Same, for the pointer wherever it currently sits — used after a rebuild,
/// which moves the cards out from under a cursor that never moved.
unsafe fn resync_hover(mc: &mut MissionControl) {
    let mut pt: POINT = std::mem::zeroed();
    if GetCursorPos(&mut pt) == 0 || ScreenToClient(mc.hwnd, &mut pt) == 0 {
        mc.hovered_space = None;
        mc.hovered_window = None;
        mc.hovered_plus = false;
        mc.hovered_close = None;
        return;
    }
    hover_at(mc, pt);
}

pub fn hide_mission_control() {
    unsafe {
        MC_STATE.with(|s| {
            let mut mc = s.borrow_mut();
            if !mc.is_visible {
                return;
            }

            for card in &mc.window_cards {
                if card.h_thumb != 0 {
                    DwmUnregisterThumbnail(card.h_thumb);
                }
            }
            mc.window_cards.clear();
            mc.space_cards.clear();

            if !mc.hwnd.is_null() {
                ShowWindow(mc.hwnd, SW_HIDE);
            }

            mc.is_visible = false;
            mc.dragging_window = None;
            mc.drag_active = false;
            mc.dragging_space = None;
            mc.drag_space_active = false;
            mc.drag_space_current_x = 0;
            mc.drag_space_target_slot = None;
            mc.hovered_close = None;
            mc.hovered_plus = false;
            log_info!("Mission Control hidden");
        });
    }
}

unsafe extern "system" fn mc_wnd_proc(
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
            // Render the scene into a memory bitmap and blit it in one
            // operation: painting straight to the screen DC shows the
            // background clear before the cards land — a visible flash on
            // every hover change. BitBlt copies all 32 bits, so the zero
            // alpha GDI writes (which lets the acrylic backdrop through)
            // survives the round-trip unchanged.
            //
            // The buffer covers `ps.rcPaint`, not the whole client area: this
            // window is monitor-sized, so a full-screen bitmap per paint would
            // make every hover tint and every space-drag frame cost a 4K
            // allocation. Shifting the viewport origin lets
            // `render_mission_control` keep drawing in absolute client
            // coordinates while GDI clips everything outside the update rect.
            let rc = ps.rcPaint;
            let width = rc.right - rc.left;
            let height = rc.bottom - rc.top;
            if width <= 0 || height <= 0 {
                EndPaint(hwnd, &ps);
                return 0;
            }
            let mem_dc = CreateCompatibleDC(hdc);
            let mem_bmp = CreateCompatibleBitmap(hdc, width, height);
            if !mem_dc.is_null() && !mem_bmp.is_null() {
                let old_bmp = SelectObject(mem_dc, mem_bmp as _);
                SetViewportOrgEx(mem_dc, -rc.left, -rc.top, std::ptr::null_mut());
                render_mission_control(mem_dc, hwnd);
                BitBlt(
                    hdc, rc.left, rc.top, width, height, mem_dc, rc.left, rc.top, SRCCOPY,
                );
                SelectObject(mem_dc, old_bmp);
            } else {
                render_mission_control(hdc, hwnd);
            }
            if !mem_bmp.is_null() {
                DeleteObject(mem_bmp as _);
            }
            if !mem_dc.is_null() {
                DeleteDC(mem_dc);
            }
            EndPaint(hwnd, &ps);
            0
        }
        WM_KEYDOWN => {
            let key = wparam as u32;
            if key == VK_ESCAPE as u32 {
                // Only a *live* drag swallows Esc. A button merely held down
                // over a card is not a drag, and eating Esc for it would leave
                // the user unable to dismiss the overlay.
                let mut cancelled_drag = false;
                MC_STATE.with(|s| {
                    let mut mc = s.borrow_mut();
                    if mc.drag_space_active {
                        mc.dragging_space = None;
                        mc.drag_space_active = false;
                        mc.drag_space_target_slot = None;
                        cancelled_drag = true;
                    }
                    if mc.drag_active {
                        if let Some(w_idx) = mc.dragging_window.take() {
                            restore_thumbnail(&mc.window_cards[w_idx]);
                        }
                        mc.drag_active = false;
                        cancelled_drag = true;
                    }
                });
                if cancelled_drag {
                    ReleaseCapture();
                    InvalidateRect(hwnd, std::ptr::null(), 0);
                } else {
                    hide_mission_control();
                }
            } else if (key == VK_LEFT as u32 || key == VK_RIGHT as u32)
                && GetKeyState(VK_CONTROL as i32) < 0
                && GetKeyState(VK_SHIFT as i32) < 0
            {
                // Keyboard equivalent of dragging a space card: Ctrl+Shift+←/→
                // walks the *active* space one slot. Ignored mid-drag so the
                // pointer and the keyboard cannot fight over the same move.
                let (mon_idx, dragging) = MC_STATE.with(|s| {
                    let mc = s.borrow();
                    (mc.active_mon_idx, mc.dragging_space)
                });
                if dragging.is_none() {
                    let delta = if key == VK_LEFT as u32 { -1 } else { 1 };
                    crate::with_app_state(|state| {
                        let Some(mon) = state.desktop_mgr.monitors.get(mon_idx) else {
                            return;
                        };
                        let (current, count) = (mon.current, mon.desktops.len());
                        if let Some(target) = neighbor_slot(current, delta, count) {
                            crate::reorder_space_on(state, mon_idx, current, target);
                        }
                    });
                }
            } else if (0x31..=0x39).contains(&key)
                || (VK_NUMPAD1 as u32..=VK_NUMPAD9 as u32).contains(&key)
            {
                let desk_idx = if key >= VK_NUMPAD1 as u32 {
                    (key - VK_NUMPAD1 as u32) as usize
                } else {
                    (key - 0x31) as usize
                };
                // Switch the monitor Mission Control is showing, not wherever
                // the cursor happens to be at keypress time — and stay open,
                // like the space-card click. Digits past this monitor's count
                // are no-ops.
                let mon_idx = MC_STATE.with(|s| s.borrow().active_mon_idx);
                crate::with_app_state(|state| {
                    if mon_idx < state.desktop_mgr.monitors.len()
                        && desk_idx < state.desktop_mgr.monitors[mon_idx].desktops.len()
                        && state.desktop_mgr.monitors[mon_idx].current != desk_idx
                    {
                        state.desktop_mgr.switch_desktop(mon_idx, desk_idx, None);
                        refresh_mission_control(state);
                    }
                });
            }
            0
        }
        // Trailing edge of the space-drag throttle: the pointer stopped inside
        // the frame budget, so the last position still owes a paint.
        WM_TIMER if wparam == TIMER_DRAG_PAINT => {
            KillTimer(hwnd, TIMER_DRAG_PAINT);
            MC_STATE.with(|s| {
                let mut mc = s.borrow_mut();
                if !mc.drag_space_active {
                    return;
                }
                mc.drag_paint_tick = GetTickCount();
                let mut client_rect: RECT = std::mem::zeroed();
                GetClientRect(hwnd, &mut client_rect);
                let width = client_rect.right - client_rect.left;
                let bar =
                    spaces_bar_metrics(mc.space_cards.len(), mc.plus_visible, width, mc.scale);
                let strip = spaces_bar_strip_rect(&bar, width, mc.scale);
                InvalidateRect(hwnd, &strip, 0);
            });
            0
        }
        // Capture can be yanked away by the system (a foreign window grabbing
        // it, a display change) with no button-up ever arriving. Without this
        // the overlay would sit frozen in its drag preview until the next
        // click. `WM_LBUTTONUP` releases capture only after it has taken the
        // drag state, so this never eats a legitimate drop.
        WM_CAPTURECHANGED => {
            let mut cancelled = false;
            MC_STATE.with(|s| {
                // Hiding the overlay releases capture from inside a live
                // `MC_STATE` borrow, so this message can arrive re-entrantly.
                // Whoever holds the state is already tearing the drag down.
                let Ok(mut mc) = s.try_borrow_mut() else {
                    return;
                };
                if mc.dragging_space.take().is_some() {
                    cancelled = mc.drag_space_active;
                    mc.drag_space_active = false;
                    mc.drag_space_target_slot = None;
                }
                if let Some(w_idx) = mc.dragging_window.take() {
                    if mc.drag_active {
                        restore_thumbnail(&mc.window_cards[w_idx]);
                        cancelled = true;
                    }
                    mc.drag_active = false;
                }
            });
            if cancelled {
                InvalidateRect(hwnd, std::ptr::null(), 0);
            }
            0
        }
        WM_LBUTTONDOWN => {
            let pt = POINT {
                x: (lparam & 0xFFFF) as i16 as i32,
                y: ((lparam >> 16) & 0xFFFF) as i16 as i32,
            };
            let mut action_add: Option<usize> = None;
            let mut action_remove: Option<(usize, usize)> = None;
            let mut should_hide = false;

            MC_STATE.with(|s| {
                let mut mc = s.borrow_mut();
                // The "+" tile lives outside space_cards; test it first.
                if mc.plus_visible && pt_in_rect(&mc.plus_rect, pt) {
                    action_add = Some(mc.active_mon_idx);
                    return;
                }

                // Close buttons win over the card body beneath them —
                // otherwise the click would switch to the space instead of
                // removing it. Only offered while more than one space exists.
                if mc.space_cards.len() > 1 {
                    for card in &mc.space_cards {
                        if pt_in_rect(&close_button_rect(&card.rect, mc.scale), pt) {
                            action_remove = Some((mc.active_mon_idx, card.desk_idx));
                            return;
                        }
                    }
                }

                // Check spaces bar click / drag start.
                for (idx, card) in mc.space_cards.iter().enumerate() {
                    if pt_in_rect(&card.rect, pt) {
                        mc.dragging_space = Some(idx);
                        mc.drag_space_active = false;
                        mc.drag_offset = pt;
                        mc.drag_space_current_x = pt.x;
                        mc.drag_space_target_slot = Some(idx);
                        SetCapture(hwnd);
                        return;
                    }
                }

                // Check window card click / drag start
                for (idx, card) in mc.window_cards.iter().enumerate() {
                    if pt_in_rect(&card.card_rect, pt) {
                        mc.dragging_window = Some(idx);
                        mc.drag_active = false;
                        mc.drag_offset = pt;
                        SetCapture(hwnd);
                        return;
                    }
                }

                // Clicked backdrop -> dismiss
                should_hide = true;
            });

            if let Some(mon) = action_add {
                // The choke point persists the count, re-registers hotkeys
                // and refreshes the open overlay.
                crate::with_app_state(|state| crate::add_space_on(state, mon));
            } else if let Some((mon, desk)) = action_remove {
                crate::with_app_state(|state| crate::remove_space_on(state, mon, desk));
            } else if should_hide {
                hide_mission_control();
            }
            0
        }
        WM_MOUSEMOVE => {
            let pt = POINT {
                x: (lparam & 0xFFFF) as i16 as i32,
                y: ((lparam >> 16) & 0xFFFF) as i16 as i32,
            };
            MC_STATE.with(|s| {
                let mut mc = s.borrow_mut();
                let old_hover_s = mc.hovered_space;
                let old_hover_w = mc.hovered_window;
                let old_hover_plus = mc.hovered_plus;
                let old_hover_close = mc.hovered_close;

                // Handle space card dragging:
                if let Some(drag_s_idx) = mc.dragging_space {
                    if !mc.drag_space_active {
                        // Reordering is horizontal only, so only horizontal
                        // travel may lift the card — a vertical nudge would
                        // otherwise detach it for a gesture that can never
                        // change the order.
                        let threshold_x = GetSystemMetrics(SM_CXDRAG).max(4);
                        if (pt.x - mc.drag_offset.x).abs() > threshold_x && mc.space_cards.len() > 1
                        {
                            mc.drag_space_active = true;
                            mc.hovered_space = None;
                            mc.hovered_close = None;
                            mc.drag_paint_tick = 0;
                            mc.drag_frame_ms = frame_interval_ms(hwnd);
                        }
                    }
                    if mc.drag_space_active {
                        let Some(orig_left) = mc.space_cards.get(drag_s_idx).map(|c| c.rect.left)
                        else {
                            return;
                        };
                        let count = mc.space_cards.len();
                        let scale = mc.scale;
                        let mut client_rect: RECT = std::mem::zeroed();
                        GetClientRect(hwnd, &mut client_rect);
                        let width = client_rect.right - client_rect.left;
                        let bar = spaces_bar_metrics(count, mc.plus_visible, width, scale);

                        let card_drag_left = orig_left + (pt.x - mc.drag_offset.x);
                        let card_center_x = card_drag_left + bar.card_w / 2;

                        let target = target_slot_for_center(
                            card_center_x,
                            bar.start_x,
                            bar.card_w,
                            bar.gap,
                            count,
                        );
                        // Nothing outside the spaces bar moves while a card is
                        // dragged, so the strip is the whole dirty region —
                        // and it is *always* the whole strip, never just the
                        // band between two positions. Frames get dropped by
                        // the throttle below, so the last painted position is
                        // not the previous message's position, and a band
                        // measured from the latter leaves the pixels the card
                        // actually vacated unpainted: a trail of ghost cards.
                        let moved = mc.drag_space_current_x != pt.x
                            || mc.drag_space_target_slot != Some(target);
                        mc.drag_space_current_x = pt.x;
                        mc.drag_space_target_slot = Some(target);

                        if moved {
                            // One blit per displayed frame at most. Anything
                            // faster and DWM composites a surface mid-update,
                            // tearing the card across a scanline. Every paint
                            // covers the whole strip, so a dropped frame is
                            // simply flushed by the next one.
                            let now = GetTickCount();
                            if now.wrapping_sub(mc.drag_paint_tick) >= mc.drag_frame_ms {
                                mc.drag_paint_tick = now;
                                KillTimer(hwnd, TIMER_DRAG_PAINT);
                                let strip = spaces_bar_strip_rect(&bar, width, scale);
                                InvalidateRect(hwnd, &strip, 0);
                            } else {
                                // Land the last position even if the pointer
                                // goes quiet inside the frame budget.
                                SetTimer(hwnd, TIMER_DRAG_PAINT, mc.drag_frame_ms, None);
                            }
                        }
                        return;
                    }
                }

                // Normal hover tracking when not dragging spaces:
                hover_at(&mut mc, pt);

                if let Some(drag_idx) = mc.dragging_window {
                    if !mc.drag_active {
                        let threshold_x = GetSystemMetrics(SM_CXDRAG).max(4);
                        let threshold_y = GetSystemMetrics(SM_CYDRAG).max(4);
                        if (pt.x - mc.drag_offset.x).abs() > threshold_x
                            || (pt.y - mc.drag_offset.y).abs() > threshold_y
                        {
                            mc.drag_active = true;
                            mc.hovered_window = None;
                            // Once per drag: the source card dims, so the
                            // whole scene legitimately changes.
                            InvalidateRect(hwnd, std::ptr::null(), 0);
                        }
                    }
                    if mc.drag_active {
                        update_drag_ghost(&mc, drag_idx, pt, hwnd);
                    }
                }

                // A hover transition only changes two cards; invalidating
                // the whole monitor-sized window repaints the entire scene
                // and reads as a flash.
                if old_hover_s != mc.hovered_space {
                    for idx in [old_hover_s, mc.hovered_space].into_iter().flatten() {
                        if let Some(card) = mc.space_cards.get(idx) {
                            invalidate_hover_rect(hwnd, &card.rect);
                        }
                    }
                }
                if old_hover_w != mc.hovered_window {
                    for idx in [old_hover_w, mc.hovered_window].into_iter().flatten() {
                        if let Some(card) = mc.window_cards.get(idx) {
                            invalidate_hover_rect(hwnd, &card.card_rect);
                        }
                    }
                }
                if old_hover_plus != mc.hovered_plus {
                    invalidate_hover_rect(hwnd, &mc.plus_rect);
                }
                // The close button only changes tint; repaint its owning card.
                if old_hover_close != mc.hovered_close {
                    for idx in [old_hover_close, mc.hovered_close].into_iter().flatten() {
                        if let Some(card) = mc.space_cards.get(idx) {
                            invalidate_hover_rect(hwnd, &card.rect);
                        }
                    }
                }
            });
            0
        }
        WM_LBUTTONUP => {
            let pt = POINT {
                x: (lparam & 0xFFFF) as i16 as i32,
                y: ((lparam >> 16) & 0xFFFF) as i16 as i32,
            };
            let mut move_window_action: Option<(HWND, usize, usize)> = None;
            let mut new_space_action: Option<(HWND, usize)> = None;
            let mut focus_window_action: Option<HWND> = None;
            let mut switch_space_action: Option<(usize, usize)> = None;
            let mut reorder_space_action: Option<(usize, usize, usize)> = None;

            MC_STATE.with(|s| {
                let mut mc = s.borrow_mut();

                // Space Card Drag & Drop or Click-to-switch
                if let Some(drag_s_idx) = mc.dragging_space.take() {
                    let was_drag = mc.drag_space_active;
                    mc.drag_space_active = false;
                    let target_slot = mc.drag_space_target_slot.take().unwrap_or(drag_s_idx);
                    let Some(card_desk) = mc.space_cards.get(drag_s_idx).map(|c| c.desk_idx) else {
                        return;
                    };

                    if was_drag {
                        // Only x decides the drop: the card is pinned to the
                        // bar while dragging, but the pointer is free to roam
                        // anywhere over the overlay without losing the move.
                        if drag_s_idx != target_slot {
                            // `space_cards` mirrors `mon.desktops` one for one
                            // (see `rebuild_cards`), so the card's slot *is* its
                            // desktop index — both arguments of `reorder_space`
                            // live in the same domain.
                            reorder_space_action =
                                Some((mc.active_mon_idx, drag_s_idx, target_slot));
                        } else {
                            InvalidateRect(hwnd, std::ptr::null(), 0);
                        }
                        return;
                    }

                    // Plain click on space card -> switch space if not active!
                    if card_desk != mc.active_desk_idx {
                        switch_space_action = Some((mc.active_mon_idx, card_desk));
                    }
                    return;
                }

                if let Some(drag_idx) = mc.dragging_window.take() {
                    let was_drag = mc.drag_active;
                    mc.drag_active = false;
                    let dragged_hwnd = mc.window_cards[drag_idx].hwnd;

                    // Dropped onto the "+" tile -> new space with this window
                    // on it (macOS parity).
                    if mc.plus_visible && pt_in_rect(&mc.plus_rect, pt) {
                        new_space_action = Some((dragged_hwnd, mc.active_mon_idx));
                        return;
                    }

                    // If dropped onto a Space card -> Move Window to that
                    // Space! The target is the card's desk_idx, not its
                    // position in the vector — those agree only while the
                    // vector is exactly the spaces in order.
                    if let Some(target_desk) = mc
                        .space_cards
                        .iter()
                        .find(|c| pt_in_rect(&c.rect, pt))
                        .map(|c| c.desk_idx)
                    {
                        move_window_action = Some((dragged_hwnd, mc.active_mon_idx, target_desk));
                        return;
                    }

                    if was_drag {
                        // Dropped anywhere else: cancel — snap the ghost
                        // thumbnail back into its grid slot.
                        restore_thumbnail(&mc.window_cards[drag_idx]);
                        InvalidateRect(hwnd, std::ptr::null(), 0);
                        return;
                    }

                    // Plain click on window card -> Focus Window & Exit!
                    if pt_in_rect(&mc.window_cards[drag_idx].card_rect, pt) {
                        focus_window_action = Some(dragged_hwnd);
                    }
                }
            });

            // After the drag state is taken, never before: `ReleaseCapture`
            // dispatches `WM_CAPTURECHANGED` to this very wndproc, and that
            // handler clears both drag state machines.
            ReleaseCapture();

            if let Some((mon, from_desk, to_desk)) = reorder_space_action {
                crate::with_app_state(|state| {
                    crate::reorder_space_on(state, mon, from_desk, to_desk)
                });
            } else if let Some((mon, desk)) = switch_space_action {
                crate::with_app_state(|state| {
                    state.desktop_mgr.switch_desktop(mon, desk, None);
                    refresh_mission_control(state);
                });
            } else if let Some((target_hwnd, mon_idx, target_desk)) = move_window_action {
                log_info!(
                    "Mission Control Drag&Drop: Moved window {:?} to Space {}",
                    target_hwnd,
                    target_desk + 1
                );
                crate::with_app_state(|state| {
                    state
                        .desktop_mgr
                        .track_window(target_hwnd, mon_idx, target_desk);
                    refresh_mission_control(state);
                });
            } else if let Some((target_hwnd, mon_idx)) = new_space_action {
                crate::with_app_state(|state| {
                    let old_max = state.desktop_mgr.max_space_count();
                    if state.desktop_mgr.add_space(mon_idx) {
                        let new_last = state.desktop_mgr.monitors[mon_idx].desktops.len() - 1;
                        log_info!(
                            "Mission Control Drag&Drop: window {:?} to new Space {}",
                            target_hwnd,
                            new_last + 1
                        );
                        state
                            .desktop_mgr
                            .track_window(target_hwnd, mon_idx, new_last);
                        crate::after_space_count_change(state, old_max);
                    } else {
                        // At the cap (defensive; the tile is hidden then):
                        // rebuilding restores the ghost to its grid slot.
                        refresh_mission_control(state);
                    }
                });
            } else if let Some(focus_hwnd) = focus_window_action {
                hide_mission_control();
                SetForegroundWindow(focus_hwnd);
            }
            0
        }
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

unsafe fn render_mission_control(hdc: windows_sys::Win32::Graphics::Gdi::HDC, hwnd: HWND) {
    MC_STATE.with(|s| {
        let mc = s.borrow();
        let mut client_rect: RECT = std::mem::zeroed();
        GetClientRect(hwnd, &mut client_rect);

        let scale = mc.scale;
        let px = |val: i32| (val as f32 * scale).round() as i32;

        // 1. Dark Acrylic Background Fill
        let bg_brush = CreateSolidBrush(rgb(0x14, 0x14, 0x18));
        FillRect(hdc, &client_rect, bg_brush);
        DeleteObject(bg_brush);

        SetBkMode(hdc, TRANSPARENT as i32);

        // 2. Render Spaces Bar (Top)
        let card_bg = CreateSolidBrush(rgb(0x1F, 0x1F, 0x24));
        let card_active_bg = CreateSolidBrush(rgb(0x28, 0x2A, 0x40)); // Accent Indigo tint
        let card_hover_bg = CreateSolidBrush(rgb(0x2C, 0x2C, 0x36));

        let placeholder_bg = CreateSolidBrush(rgb(0x16, 0x16, 0x1C)); // Vacated drop slot
        let plus_bg = CreateSolidBrush(rgb(0x17, 0x17, 0x1D)); // "New Space" tile
        let plus_hover_bg = CreateSolidBrush(rgb(0x1E, 0x20, 0x30)); // Indigo-tinted

        let pen_border = CreatePen(PS_SOLID, 1, rgb(0x38, 0x38, 0x42));
        let pen_active = CreatePen(PS_SOLID, px(2), rgb(0x81, 0x8C, 0xF8)); // Indigo Accent Outline
        let pen_drag_target = CreatePen(PS_SOLID, px(2), rgb(0x34, 0xD3, 0x99)); // Emerald Green
        let pen_placeholder = CreatePen(PS_SOLID, 1, rgb(0x48, 0x48, 0x58));
        // Dashed: the tile is an empty slot waiting to be filled, not a card.
        let pen_plus = CreatePen(PS_DASH, 1, rgb(0x4A, 0x4A, 0x58));
        let close_bg = CreateSolidBrush(rgb(0x3A, 0x3A, 0x44));
        let close_bg_hover = CreateSolidBrush(rgb(0xE8, 0x55, 0x5A)); // Destructive Red

        let r_corner = px(12);

        // Deferred to the very end of the bar so nothing overlaps it.
        let mut floating_card: Option<(usize, RECT)> = None;

        if mc.drag_space_active {
            let from_idx = mc.dragging_space.unwrap_or(0);
            let target_slot = mc.drag_space_target_slot.unwrap_or(from_idx);
            let count = mc.space_cards.len();
            let width = client_rect.right - client_rect.left;
            let bar = spaces_bar_metrics(count, mc.plus_visible, width, scale);

            let slot_rect = |slot: usize| {
                let left = bar.start_x + slot as i32 * (bar.card_w + bar.gap);
                RECT {
                    left,
                    top: bar.top_y,
                    right: left + bar.card_w,
                    bottom: bar.top_y + bar.card_h,
                }
            };

            // 2a. Placeholder Slot at target_slot
            draw_space_card(
                hdc,
                &slot_rect(target_slot),
                placeholder_bg,
                pen_placeholder,
                r_corner,
            );

            // 2b. Non-dragged cards shifted to their new slots
            for (idx, card) in mc.space_cards.iter().enumerate() {
                if idx == from_idx {
                    continue;
                }
                let slot_idx =
                    crate::desktop::remap_index_after_reorder(idx, from_idx, target_slot);
                let card_rect = slot_rect(slot_idx);

                let (brush, pen) = if card.is_active {
                    (card_active_bg, pen_active)
                } else {
                    (card_bg, pen_border)
                };
                draw_space_card(hdc, &card_rect, brush, pen, r_corner);
                draw_space_card_text(hdc, &mc, card, &card_rect, scale);
            }

            // 2c. The dragged card itself is drawn last of all — after the
            // "new space" tile below — so it passes *over* every other tile in
            // the bar instead of sliding underneath one. It tracks the cursor
            // horizontally but stays pinned to the bar's y: DWM composites the
            // thumbnail grid *above* this window's GDI output, so a card
            // dragged down over the grid would vanish behind it.
            if mc.space_cards.get(from_idx).is_some() {
                let delta_x = mc.drag_space_current_x - mc.drag_offset.x;
                let drag_left = floating_card_left(
                    mc.space_cards[from_idx].rect.left,
                    delta_x,
                    bar.card_w,
                    width,
                    scale,
                );
                let drag_top = bar.top_y - px(4);
                floating_card = Some((
                    from_idx,
                    RECT {
                        left: drag_left,
                        top: drag_top,
                        right: drag_left + bar.card_w,
                        bottom: drag_top + bar.card_h,
                    },
                ));
            }
        } else {
            for (idx, card) in mc.space_cards.iter().enumerate() {
                let is_hover = mc.hovered_space == Some(idx);
                let is_drag_target = mc.drag_active && is_hover;

                let brush = if is_drag_target {
                    card_hover_bg
                } else if card.is_active {
                    card_active_bg
                } else if is_hover {
                    card_hover_bg
                } else {
                    card_bg
                };

                let pen = if is_drag_target {
                    pen_drag_target
                } else if card.is_active {
                    pen_active
                } else {
                    pen_border
                };

                draw_space_card(hdc, &card.rect, brush, pen, r_corner);
                draw_space_card_text(hdc, &mc, card, &card.rect, scale);

                // Close button, macOS style: only on the hovered card, and never
                // when this is the monitor's last space.
                if is_hover && mc.space_cards.len() > 1 {
                    let cb = close_button_rect(&card.rect, scale);
                    let is_close_hover = mc.hovered_close == Some(idx);
                    SelectObject(
                        hdc,
                        if is_close_hover {
                            close_bg_hover
                        } else {
                            close_bg
                        },
                    );
                    SelectObject(hdc, pen_border);
                    let d = cb.right - cb.left;
                    // Corner radius = diameter renders the round rect as a circle.
                    RoundRect(hdc, cb.left, cb.top, cb.right, cb.bottom, d, d);
                    SelectObject(hdc, mc.h_font_close);
                    SetTextColor(hdc, rgb(0xFF, 0xFF, 0xFF));
                    let mut x_rect = cb;
                    draw_text_wide(
                        hdc,
                        "✕",
                        &mut x_rect,
                        DT_CENTER | DT_SINGLELINE | DT_VCENTER,
                    );
                }
            }
        }

        // The "new space" tile: deliberately *not* in the space cards' visual
        // family. A dimmer, dashed ghost slot reads as an action at the end of
        // the row rather than an N+1'th space — the old flat "+" in card
        // colours was too easy to mistake for one. Emerald drag outline when a
        // window drag hovers it (drop = new space + move).
        if mc.plus_visible {
            let is_hover = mc.hovered_plus;
            let is_drag_target = mc.drag_active && is_hover;
            let brush = if is_hover { plus_hover_bg } else { plus_bg };
            let pen = if is_drag_target {
                pen_drag_target
            } else if is_hover {
                pen_active
            } else {
                pen_plus
            };
            draw_space_card(hdc, &mc.plus_rect, brush, pen, r_corner);

            SetTextColor(
                hdc,
                if is_drag_target {
                    rgb(0x34, 0xD3, 0x99)
                } else if is_hover {
                    rgb(0xC7, 0xD2, 0xFE)
                } else {
                    rgb(0x8A, 0x8A, 0x96)
                },
            );

            // Same Fluent "Add" mark the tray menu's New Space item uses.
            SelectObject(hdc, mc.h_font_glyph);
            let glyph = char::from_u32(crate::menu::GLYPH_ADD as u32)
                .map(String::from)
                .unwrap_or_else(|| "+".to_string());
            let mut glyph_rect = RECT {
                left: mc.plus_rect.left,
                top: mc.plus_rect.top + px(20),
                right: mc.plus_rect.right,
                bottom: mc.plus_rect.top + px(56),
            };
            draw_text_wide(
                hdc,
                &glyph,
                &mut glyph_rect,
                DT_CENTER | DT_SINGLELINE | DT_VCENTER,
            );

            SelectObject(hdc, mc.h_font_small);
            let mut label_rect = RECT {
                left: mc.plus_rect.left + px(4),
                top: mc.plus_rect.top + px(56),
                right: mc.plus_rect.right - px(4),
                bottom: mc.plus_rect.bottom - px(12),
            };
            draw_text_wide(
                hdc,
                "New Space",
                &mut label_rect,
                DT_CENTER | DT_SINGLELINE | DT_VCENTER | DT_END_ELLIPSIS,
            );
        }

        // Last in the bar, so a dragged card rides over the tiles it passes.
        if let Some((idx, drag_rect)) = floating_card {
            if let Some(card) = mc.space_cards.get(idx) {
                draw_space_card(hdc, &drag_rect, card_active_bg, pen_active, r_corner);
                draw_space_card_text(hdc, &mc, card, &drag_rect, scale);
            }
        }

        DeleteObject(plus_bg);
        DeleteObject(plus_hover_bg);
        DeleteObject(pen_plus);
        DeleteObject(card_bg);
        DeleteObject(card_active_bg);
        DeleteObject(card_hover_bg);
        DeleteObject(placeholder_bg);
        DeleteObject(pen_border);
        DeleteObject(pen_active);
        DeleteObject(pen_drag_target);
        DeleteObject(pen_placeholder);
        DeleteObject(close_bg);
        DeleteObject(close_bg_hover);

        // 3. Render Window Cards (Exposé Grid)
        if mc.window_cards.is_empty() {
            SelectObject(hdc, mc.h_font_title);
            SetTextColor(hdc, rgb(0x71, 0x71, 0x7A));
            let empty_msg = format!("No open windows on Space {}", mc.active_desk_idx + 1);
            let mut center_rect = RECT {
                left: client_rect.left,
                top: client_rect.top + px(220),
                right: client_rect.right,
                bottom: client_rect.bottom,
            };
            draw_text_wide(
                hdc,
                &empty_msg,
                &mut center_rect,
                DT_CENTER | DT_SINGLELINE | DT_VCENTER,
            );
            return;
        }

        let win_card_bg = CreateSolidBrush(rgb(0x1F, 0x1F, 0x24));
        let win_card_hover_bg = CreateSolidBrush(rgb(0x28, 0x28, 0x32));
        let win_pen = CreatePen(PS_SOLID, 1, rgb(0x38, 0x38, 0x42));
        let win_pen_hover = CreatePen(PS_SOLID, px(2), rgb(0x81, 0x8C, 0xF8)); // Indigo Highlight

        let card_corner = px(14);
        let icon_size = px(20);

        for (idx, card) in mc.window_cards.iter().enumerate() {
            let is_dragged = mc.drag_active && mc.dragging_window == Some(idx);
            let is_hover = !is_dragged && mc.hovered_window == Some(idx);
            let brush = if is_hover {
                win_card_hover_bg
            } else {
                win_card_bg
            };
            let pen = if is_hover { win_pen_hover } else { win_pen };

            SelectObject(hdc, brush);
            SelectObject(hdc, pen);
            RoundRect(
                hdc,
                card.card_rect.left,
                card.card_rect.top,
                card.card_rect.right,
                card.card_rect.bottom,
                card_corner,
                card_corner,
            );

            // Draw Real Window Icon if available
            let header_h = px(38);
            let icon_x = card.card_rect.left + px(12);
            let icon_y = card.card_rect.top + (header_h - icon_size) / 2;
            if !card.h_icon.is_null() {
                DrawIconEx(
                    hdc,
                    icon_x,
                    icon_y,
                    card.h_icon,
                    icon_size,
                    icon_size,
                    0,
                    null_mut(),
                    DI_NORMAL,
                );
            }

            // Window Header Title Text (dimmed on the lifted source card)
            SelectObject(hdc, mc.h_font_card);
            let title_color = if is_dragged {
                rgb(0x71, 0x71, 0x7A)
            } else {
                rgb(0xFF, 0xFF, 0xFF)
            };
            SetTextColor(hdc, title_color);
            let text_left = if !card.h_icon.is_null() {
                icon_x + icon_size + px(8)
            } else {
                card.card_rect.left + px(14)
            };

            let mut title_r = RECT {
                left: text_left,
                top: card.card_rect.top,
                right: card.card_rect.right - px(14),
                bottom: card.card_rect.top + header_h,
            };
            draw_text_wide(
                hdc,
                &card.title,
                &mut title_r,
                DT_SINGLELINE | DT_VCENTER | DT_END_ELLIPSIS,
            );
        }

        DeleteObject(win_card_bg);
        DeleteObject(win_card_hover_bg);
        DeleteObject(win_pen);
        DeleteObject(win_pen_hover);
    });
}

/// While a drag is active the card's live DWM thumbnail doubles as the drag
/// ghost: its destination rect is retargeted to a scaled-down rect that
/// follows the cursor, at reduced opacity. GPU-composited, so no GDI
/// flicker and the "ghost" stays a live video of the window.
unsafe fn update_drag_ghost(mc: &MissionControl, drag_idx: usize, pt: POINT, hwnd: HWND) {
    let card = &mc.window_cards[drag_idx];
    if card.h_thumb == 0 {
        return;
    }

    const GHOST_SCALE: f32 = 0.4;
    let src_w = (card.thumb_rect.right - card.thumb_rect.left).max(1);
    let src_h = (card.thumb_rect.bottom - card.thumb_rect.top).max(1);
    let ghost_w = ((src_w as f32 * GHOST_SCALE) as i32).max(48);
    let ghost_h = ((src_h as f32 * GHOST_SCALE) as i32).max(32);

    let mut client_rect: RECT = std::mem::zeroed();
    GetClientRect(hwnd, &mut client_rect);

    let left = (pt.x - ghost_w / 2)
        .max(client_rect.left)
        .min(client_rect.right - ghost_w);
    let top = (pt.y - ghost_h / 2)
        .max(client_rect.top)
        .min(client_rect.bottom - ghost_h);

    let mut props: DWM_THUMBNAIL_PROPERTIES = std::mem::zeroed();
    props.dwFlags = DWM_TNP_RECTDESTINATION | DWM_TNP_VISIBLE | DWM_TNP_OPACITY;
    props.rcDestination = RECT {
        left,
        top,
        right: left + ghost_w,
        bottom: top + ghost_h,
    };
    props.fVisible = 1;
    props.opacity = 200;
    DwmUpdateThumbnailProperties(card.h_thumb, &props);
}

/// Snap a card's thumbnail back into its grid slot at full opacity after a
/// cancelled drag.
unsafe fn restore_thumbnail(card: &WindowCard) {
    if card.h_thumb == 0 {
        return;
    }
    let mut props: DWM_THUMBNAIL_PROPERTIES = std::mem::zeroed();
    props.dwFlags = DWM_TNP_RECTDESTINATION | DWM_TNP_VISIBLE | DWM_TNP_OPACITY;
    props.rcDestination = card.thumb_rect;
    props.fVisible = 1;
    props.opacity = 255;
    DwmUpdateThumbnailProperties(card.h_thumb, &props);
}

fn pt_in_rect(rect: &RECT, pt: POINT) -> bool {
    pt.x >= rect.left && pt.x <= rect.right && pt.y >= rect.top && pt.y <= rect.bottom
}

/// Resolved geometry for the spaces bar strip.
struct SpacesBarMetrics {
    card_w: i32,
    card_h: i32,
    gap: i32,
    start_x: i32,
    top_y: i32,
    /// Zeroed when `has_plus` was false.
    plus_rect: RECT,
}

/// Centered-strip layout for `count` space cards plus an optional "+" tile.
/// Pure so the overflow clamp is unit-testable: when the natural width would
/// not fit the monitor (9 cards on a narrow display at high DPI), card width
/// shrinks toward a floor instead of `start_x` going negative and pushing
/// cards off both edges.
fn spaces_bar_metrics(count: usize, has_plus: bool, width: i32, scale: f32) -> SpacesBarMetrics {
    let px = |val: i32| (val as f32 * scale).round() as i32;
    let count = count.max(1) as i32;
    let card_h = px(100);
    let gap = px(16);
    let top_y = px(28);
    // Wide enough for the glyph over its "New Space" label, still visibly
    // narrower than a space card so the row reads as "N spaces, then an
    // action" rather than N+1 spaces.
    let plus_w = px(132);
    let margin = px(60);

    let plus_total = if has_plus { plus_w + gap } else { 0 };
    let avail = width - 2 * margin;
    let mut card_w = px(210);
    if count * card_w + (count - 1) * gap + plus_total > avail {
        card_w = ((avail - plus_total - (count - 1) * gap) / count).max(px(120));
    }

    let total_w = count * card_w + (count - 1) * gap + plus_total;
    let start_x = (width - total_w) / 2;
    let plus_left = start_x + count * (card_w + gap);
    let plus_rect = if has_plus {
        RECT {
            left: plus_left,
            top: top_y,
            right: plus_left + plus_w,
            bottom: top_y + card_h,
        }
    } else {
        RECT {
            left: 0,
            top: 0,
            right: 0,
            bottom: 0,
        }
    };

    SpacesBarMetrics {
        card_w,
        card_h,
        gap,
        start_x,
        top_y,
        plus_rect,
    }
}

/// Given the horizontal center of a dragged space card, the start_x of the
/// spaces bar, the card width, gap, and total number of cards, computes which
/// slot (0..count-1) the card would land in. Transitions at the exact midpoints
/// between adjacent card slots. Pure so it is unit-testable.
fn target_slot_for_center(
    center_x: i32,
    start_x: i32,
    card_w: i32,
    gap: i32,
    count: usize,
) -> usize {
    if count <= 1 {
        return 0;
    }
    let pitch = card_w + gap;
    if pitch <= 0 {
        return 0;
    }
    let rel_x = center_x - start_x;
    let slot = (rel_x + gap / 2) / pitch;
    slot.clamp(0, (count - 1) as i32) as usize
}

/// The band of the overlay the spaces bar occupies, padded for the floating
/// card a drag lifts `px(4)` above the row. Nothing else moves while a space
/// card is dragged, so this is the only region those frames need to repaint.
/// Pure so the padding is unit-testable.
fn spaces_bar_strip_rect(bar: &SpacesBarMetrics, width: i32, scale: f32) -> RECT {
    let pad = (8.0 * scale).round() as i32;
    RECT {
        left: 0,
        top: bar.top_y - pad,
        right: width,
        bottom: bar.top_y + bar.card_h + pad,
    }
}

/// Left edge of the floating card a space drag lifts: the card's resting
/// position slid by `delta_x`, kept inside the overlay's margins. Shared by
/// the renderer and the drag handler so the region invalidated for a frame is
/// exactly the region that frame redraws. Pure so the clamp is unit-testable.
fn floating_card_left(card_left: i32, delta_x: i32, card_w: i32, width: i32, scale: f32) -> i32 {
    let margin = (20.0 * scale).round() as i32;
    let max_left = width - card_w - margin;
    // Not `clamp`: on a client area too narrow for one card plus margins
    // `max_left` drops below `margin`, and `clamp` panics when its bounds
    // cross — inside `WM_PAINT`, in a daemon.
    (card_left + delta_x).min(max_left).max(margin)
}

/// The display's frame interval in milliseconds, the budget a space drag
/// paints at most once inside of.
unsafe fn frame_interval_ms(hwnd: HWND) -> u32 {
    let hdc = GetDC(hwnd);
    if hdc.is_null() {
        return 16;
    }
    let hz = GetDeviceCaps(hdc, VREFRESH as i32);
    ReleaseDC(hwnd, hdc);
    // 0 and 1 both mean "hardware default" — assume the 60 Hz floor.
    let hz = if hz <= 1 { 60 } else { hz.min(360) };
    ((1000 / hz) as u32).max(4)
}

/// The slot `delta` steps from `current`, or `None` at the ends. Backs the
/// `Ctrl+Shift+←/→` keyboard equivalent of dragging a space card.
fn neighbor_slot(current: usize, delta: i32, count: usize) -> Option<usize> {
    let target = current as i32 + delta;
    if target < 0 || target >= count as i32 {
        return None;
    }
    Some(target as usize)
}

/// A space card's chrome: the rounded body in `brush`, outlined in `pen`.
/// Shared by the resting cards, the shifted cards a drag previews, the drop
/// placeholder and the floating card, so they can never drift apart.
unsafe fn draw_space_card(
    hdc: windows_sys::Win32::Graphics::Gdi::HDC,
    rect: &RECT,
    brush: windows_sys::Win32::Graphics::Gdi::HBRUSH,
    pen: windows_sys::Win32::Graphics::Gdi::HPEN,
    r_corner: i32,
) {
    SelectObject(hdc, brush);
    SelectObject(hdc, pen);
    RoundRect(
        hdc,
        rect.left,
        rect.top,
        rect.right,
        rect.bottom,
        r_corner,
        r_corner,
    );
}

unsafe fn draw_space_card_text(
    hdc: windows_sys::Win32::Graphics::Gdi::HDC,
    mc: &MissionControl,
    card: &SpaceCard,
    card_rect: &RECT,
    scale: f32,
) {
    let px = |val: i32| (val as f32 * scale).round() as i32;

    // Title: "Space X"
    SelectObject(hdc, mc.h_font_title);
    SetTextColor(hdc, rgb(0xFF, 0xFF, 0xFF));
    let title_text = format!("Space {}", card.desk_idx + 1);
    let mut title_rect = RECT {
        left: card_rect.left + px(16),
        top: card_rect.top + px(18),
        right: card_rect.right - px(16),
        bottom: card_rect.top + px(48),
    };
    draw_text_wide(
        hdc,
        &title_text,
        &mut title_rect,
        DT_CENTER | DT_SINGLELINE | DT_VCENTER,
    );

    // Subtitle: "N windows" or "Active"
    SelectObject(hdc, mc.h_font_small);
    let sub_color = if card.is_active {
        rgb(0xC7, 0xD2, 0xFE)
    } else {
        rgb(0x9C, 0x9C, 0xA4)
    };
    SetTextColor(hdc, sub_color);

    let sub_text = if card.is_active {
        if card.window_count == 1 {
            "Active • 1 window".to_string()
        } else {
            format!("Active • {} windows", card.window_count)
        }
    } else if card.window_count == 1 {
        "1 window".to_string()
    } else {
        format!("{} windows", card.window_count)
    };

    let mut sub_rect = RECT {
        left: card_rect.left + px(12),
        top: card_rect.top + px(52),
        right: card_rect.right - px(12),
        bottom: card_rect.bottom - px(14),
    };
    // Ellipsize: on a monitor narrow enough to push cards to their width
    // floor, "Active • 12 windows" no longer fits the card.
    draw_text_wide(
        hdc,
        &sub_text,
        &mut sub_rect,
        DT_CENTER | DT_SINGLELINE | DT_VCENTER | DT_END_ELLIPSIS,
    );
}

/// Circular close button in a space card's top-right corner. Shared by render
/// and hit-testing so the drawn button and the clickable area cannot disagree.
fn close_button_rect(card: &RECT, scale: f32) -> RECT {
    let px = |val: i32| (val as f32 * scale).round() as i32;
    let d = px(20);
    let pad = px(6);
    RECT {
        left: card.right - pad - d,
        top: card.top + pad,
        right: card.right - pad,
        bottom: card.top + pad + d,
    }
}

/// Invalidate a card's area padded by a few pixels so the hover stroke drawn
/// on the card edge is covered in both the old and new state.
unsafe fn invalidate_hover_rect(hwnd: HWND, rect: &RECT) {
    let padded = RECT {
        left: rect.left - 3,
        top: rect.top - 3,
        right: rect.right + 3,
        bottom: rect.bottom + 3,
    };
    InvalidateRect(hwnd, &padded, 0);
}

unsafe fn draw_text_wide(
    hdc: windows_sys::Win32::Graphics::Gdi::HDC,
    text: &str,
    rect: &mut RECT,
    flags: u32,
) {
    let wide: Vec<u16> = text.encode_utf16().collect();
    DrawTextW(hdc, wide.as_ptr(), wide.len() as i32, rect, flags);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect_w(r: &RECT) -> i32 {
        r.right - r.left
    }

    #[test]
    fn spaces_bar_fits_four_cards_at_natural_width() {
        let m = spaces_bar_metrics(4, true, 1920, 1.0);
        assert_eq!(m.card_w, 210);
        assert!(m.start_x > 0);
        // Plus tile sits one gap after the last card.
        assert_eq!(m.plus_rect.left, m.start_x + 4 * (m.card_w + m.gap));
        assert_eq!(rect_w(&m.plus_rect), 132);
    }

    #[test]
    fn spaces_bar_shrinks_cards_instead_of_overflowing() {
        // Nine cards at 210px + gaps exceed 1920px; the clamp must keep the
        // strip inside the margins rather than letting start_x go negative.
        let m = spaces_bar_metrics(9, false, 1920, 1.0);
        assert!(m.card_w < 210);
        assert!(m.card_w >= 120);
        assert!(m.start_x >= 0);
        let total = 9 * m.card_w + 8 * m.gap;
        assert!(total <= 1920 - 2 * 60);
    }

    #[test]
    fn spaces_bar_clamp_accounts_for_the_plus_tile() {
        // Same width: adding the plus tile must shrink cards further, never
        // push the tile past the margin.
        let without = spaces_bar_metrics(8, false, 1600, 1.0);
        let with = spaces_bar_metrics(8, true, 1600, 1.0);
        assert!(with.card_w <= without.card_w);
        assert!(with.plus_rect.right <= 1600 - 60);
    }

    #[test]
    fn spaces_bar_at_max_count_has_no_plus_rect() {
        let m = spaces_bar_metrics(9, false, 3840, 1.5);
        assert_eq!(rect_w(&m.plus_rect), 0);
    }

    #[test]
    fn close_button_sits_inside_the_card_corner() {
        let card = RECT {
            left: 100,
            top: 28,
            right: 310,
            bottom: 128,
        };
        let cb = close_button_rect(&card, 1.0);
        assert!(cb.left > card.left && cb.right <= card.right);
        assert!(cb.top >= card.top && cb.bottom < card.bottom);
        // Scale grows the button with the card.
        let cb2 = close_button_rect(&card, 2.0);
        assert_eq!(cb2.bottom - cb2.top, 2 * (cb.bottom - cb.top));
    }

    #[test]
    fn target_slot_for_center_resolves_correct_slots_and_transitions_at_midpoints() {
        // 4 cards with start_x = 100, card_w = 200, gap = 20 (pitch = 220).
        // Slot centers:
        // Slot 0: 200 (range < 310)
        // Slot 1: 420 (range 310..530)
        // Slot 2: 640 (range 530..750)
        // Slot 3: 860 (range >= 750)
        let start_x = 100;
        let card_w = 200;
        let gap = 20;
        let count = 4;

        // Centered on slot 0:
        assert_eq!(target_slot_for_center(200, start_x, card_w, gap, count), 0);
        // Offscreen / clamped left:
        assert_eq!(target_slot_for_center(-100, start_x, card_w, gap, count), 0);
        // Just before midpoint to slot 1 (309):
        assert_eq!(target_slot_for_center(309, start_x, card_w, gap, count), 0);
        // Exact midpoint to slot 1 (310):
        assert_eq!(target_slot_for_center(310, start_x, card_w, gap, count), 1);
        // Centered on slot 1 (420):
        assert_eq!(target_slot_for_center(420, start_x, card_w, gap, count), 1);
        // Just before midpoint to slot 2 (529):
        assert_eq!(target_slot_for_center(529, start_x, card_w, gap, count), 1);
        // Midpoint to slot 2 (530):
        assert_eq!(target_slot_for_center(530, start_x, card_w, gap, count), 2);
        // Centered on slot 3 (860):
        assert_eq!(target_slot_for_center(860, start_x, card_w, gap, count), 3);
        // Far right / clamped to max slot:
        assert_eq!(target_slot_for_center(2000, start_x, card_w, gap, count), 3);
    }

    #[test]
    fn bar_strip_spans_the_width_and_covers_the_lifted_card() {
        let bar = spaces_bar_metrics(4, true, 1920, 1.0);
        let strip = spaces_bar_strip_rect(&bar, 1920, 1.0);
        assert_eq!(strip.left, 0);
        assert_eq!(strip.right, 1920);
        // The floating card is drawn px(4) above the row; the strip must
        // include it, or a drag would smear its top edge.
        assert!(strip.top <= bar.top_y - 4);
        assert!(strip.bottom >= bar.top_y + bar.card_h);

        // Padding scales with DPI, like everything else in the bar.
        let bar2 = spaces_bar_metrics(4, true, 3840, 2.0);
        let strip2 = spaces_bar_strip_rect(&bar2, 3840, 2.0);
        assert_eq!(bar2.top_y - strip2.top, 2 * (bar.top_y - strip.top));
    }

    #[test]
    fn floating_card_stays_inside_the_overlay_margins() {
        // 1920-wide overlay, 210-wide card, 20px margins at scale 1.
        assert_eq!(floating_card_left(100, 50, 210, 1920, 1.0), 150);
        assert_eq!(floating_card_left(100, -500, 210, 1920, 1.0), 20);
        assert_eq!(floating_card_left(1600, 500, 210, 1920, 1.0), 1690);
        // Margins scale with DPI, like the rest of the bar.
        assert_eq!(floating_card_left(0, -500, 210, 1920, 2.0), 40);
        // A client area too narrow for one card crosses the clamp bounds; it
        // must pin to the left margin, not panic inside `WM_PAINT`.
        assert_eq!(floating_card_left(0, 999, 210, 100, 1.0), 20);
    }

    #[test]
    fn neighbor_slot_stops_at_both_ends() {
        assert_eq!(neighbor_slot(0, 1, 4), Some(1));
        assert_eq!(neighbor_slot(3, -1, 4), Some(2));
        assert_eq!(neighbor_slot(0, -1, 4), None);
        assert_eq!(neighbor_slot(3, 1, 4), None);
        // Single-space monitor: nowhere to go in either direction.
        assert_eq!(neighbor_slot(0, -1, 1), None);
        assert_eq!(neighbor_slot(0, 1, 1), None);
    }
}

unsafe fn create_segoe_font(height: i32, weight: i32) -> HFONT {
    create_font("Segoe UI Variable Display", height, weight)
}

unsafe fn create_font(face: &str, height: i32, weight: i32) -> HFONT {
    let font_name: Vec<u16> = format!("{face}\0").encode_utf16().collect();
    CreateFontW(
        height,
        0,
        0,
        0,
        weight,
        0,
        0,
        0,
        0,
        0,
        0,
        0,
        0,
        font_name.as_ptr(),
    )
}
