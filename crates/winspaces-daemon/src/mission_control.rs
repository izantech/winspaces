use crate::desktop::is_valid_window;
use crate::{log_info, log_warn};
use std::cell::RefCell;
use std::collections::HashMap;
use std::ptr::null_mut;
use windows_sys::Win32::Foundation::{BOOL, HWND, LPARAM, LRESULT, POINT, RECT, SIZE, WPARAM};
use windows_sys::Win32::Graphics::Dwm::{
    DwmQueryThumbnailSourceSize, DwmRegisterThumbnail, DwmSetWindowAttribute,
    DwmUnregisterThumbnail, DwmUpdateThumbnailProperties, DWMWA_SYSTEMBACKDROP_TYPE,
    DWMWA_USE_IMMERSIVE_DARK_MODE, DWM_THUMBNAIL_PROPERTIES, DWM_TNP_OPACITY,
    DWM_TNP_RECTDESTINATION, DWM_TNP_SOURCECLIENTAREAONLY, DWM_TNP_VISIBLE,
};
use windows_sys::Win32::Graphics::Gdi::{
    BeginPaint, BitBlt, CreateCompatibleBitmap, CreateCompatibleDC, CreateFontW, CreatePen,
    CreateSolidBrush, DeleteDC, DeleteObject, DrawTextW, EndPaint, FillRect, GetMonitorInfoW,
    InvalidateRect, RoundRect, SelectObject, SetBkMode, SetTextColor, DT_CENTER, DT_END_ELLIPSIS,
    DT_SINGLELINE, DT_VCENTER, HFONT, MONITORINFO, PAINTSTRUCT, PS_SOLID, SRCCOPY, TRANSPARENT,
};
use windows_sys::Win32::UI::HiDpi::GetDpiForWindow;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    ReleaseCapture, SetCapture, VK_ESCAPE, VK_NUMPAD1, VK_NUMPAD9,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DrawIconEx, GetClientRect, GetSystemMetrics, GetWindowTextW,
    KillTimer, RegisterClassExW, SetForegroundWindow, SetTimer, SetWindowPos, ShowWindow,
    SystemParametersInfoW, CS_HREDRAW, CS_VREDRAW, DI_NORMAL, GCLP_HICON, GCLP_HICONSM, ICON_BIG,
    ICON_SMALL, ICON_SMALL2, SM_CXDRAG, SM_CYDRAG, SWP_FRAMECHANGED, SWP_NOACTIVATE, SWP_NOZORDER,
    SW_HIDE, SW_SHOW, WM_ERASEBKGND, WM_GETICON, WM_KEYDOWN, WM_LBUTTONDOWN, WM_LBUTTONUP,
    WM_MOUSEMOVE, WM_PAINT, WM_TIMER, WNDCLASSEXW, WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_POPUP,
};
use winspaces_common::MAX_DESKTOPS;

#[allow(clippy::upper_case_acronyms)]
pub type HICON = *mut std::ffi::c_void;

const fn rgb(r: u8, g: u8, b: u8) -> u32 {
    (r as u32) | ((g as u32) << 8) | ((b as u32) << 16)
}

const MC_CLASS_NAME: &str = "WinSpacesMissionControl";

const TIMER_MC_ANIM: usize = 10; // timer ids are per-HWND; the overlay owns no other timer
const MC_ANIM_TICK_MS: u32 = 15; // matches the ~15.6 ms system tick; do not pretend to 16.67
const MC_ANIM_OPEN_MS: f32 = 200.0;
const MC_ANIM_CLOSE_MS: f32 = 160.0;
const MC_ANIM_SWITCH_MS: f32 = 160.0;
const MC_ANIM_MAX_CARDS: usize = 24; // guardrail, NOT a measured number — see risks
                                     // Not exposed by windows-sys (same precedent as WTS_* in main.rs).
const SPI_GETCLIENTAREAANIMATION: u32 = 0x1042;
// A transition that overruns its nominal duration by this much gets one
// warning line — see the "unmeasured guardrail" risk.
const MC_ANIM_OVERRUN_FACTOR: f32 = 1.5;
// Degenerate open sources (minimized / off-overlay) fall back to a zoom from
// this fraction of the final card size instead of flying in from a real rect.
const MC_ANIM_SELF_ZOOM: f32 = 0.85;

fn lerp_i32(a: i32, b: i32, t: f32) -> i32 {
    a + ((b - a) as f32 * t).round() as i32
}

fn lerp_rect(a: &RECT, b: &RECT, t: f32) -> RECT {
    RECT {
        left: lerp_i32(a.left, b.left, t),
        top: lerp_i32(a.top, b.top, t),
        right: lerp_i32(a.right, b.right, t),
        bottom: lerp_i32(a.bottom, b.bottom, t),
    }
}

// Channel-wise on the rgb() layout above: r in bits 0-7, g in bits 8-15, b in bits 16-23.
fn lerp_rgb(a: u32, b: u32, t: f32) -> u32 {
    let ar = (a & 0xFF) as i32;
    let ag = ((a >> 8) & 0xFF) as i32;
    let ab = ((a >> 16) & 0xFF) as i32;
    let br = (b & 0xFF) as i32;
    let bg = ((b >> 8) & 0xFF) as i32;
    let bb = ((b >> 16) & 0xFF) as i32;
    let r = lerp_i32(ar, br, t) as u32;
    let g = lerp_i32(ag, bg, t) as u32;
    let bl = lerp_i32(ab, bb, t) as u32;
    r | (g << 8) | (bl << 16)
}

fn ease_out_cubic(t: f32) -> f32 {
    1.0 - (1.0 - t).powi(3)
}

fn ease_in_quad(t: f32) -> f32 {
    t * t
}

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
    pub anim_from_card: RECT,
    pub anim_from_thumb: RECT,
    pub draw_rect: RECT,
}

#[derive(Clone, Copy, PartialEq)]
pub enum AnimKind {
    Open,
    Close,
    Switch,
}

/// (from, to) endpoints for the per-tick rect lerp, keyed by animation kind.
/// Open and Switch are entrances: they start at `anim_from` (the captured
/// source / slide-in offset) and arrive at `final_rect`. Close is an exit —
/// the card is already sitting at `final_rect` when it starts, and shrinks
/// back into `anim_from` (the real window position, recaptured live by
/// `hide_mission_control_focusing`). Kept pure so it is unit-testable
/// without the DWM calls the rest of `tick_animation` needs.
fn anim_endpoints(kind: AnimKind, anim_from: RECT, final_rect: RECT) -> (RECT, RECT) {
    match kind {
        AnimKind::Open | AnimKind::Switch => (anim_from, final_rect),
        AnimKind::Close => (final_rect, anim_from),
    }
}

pub struct Animation {
    kind: AnimKind,
    started: std::time::Instant,
    duration_ms: f32,
    eased: f32, // written once per tick, read by render — see the tearing note
    outgoing: Vec<WindowCard>, // the outgoing set for a Switch; empty otherwise
    push_dx: i32, // Phase 6 slide distance, signed by switch direction
    prev_active: Option<usize>,
    pending_focus: HWND, // window to raise after a Close completes
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
    pub h_font_title: HFONT,
    pub h_font_card: HFONT,
    pub h_font_small: HFONT,
    pub anim: Option<Animation>,
    pub animations_enabled: bool,
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
            h_font_title: null_mut(),
            h_font_card: null_mut(),
            h_font_small: null_mut(),
            anim: None,
            animations_enabled: true,
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

/// Commits any in-flight animation to its final state from outside the
/// module. A shutdown arriving mid-close must still run the deferred
/// DwmUnregisterThumbnail + SW_HIDE teardown, or the registration leaks.
pub fn finish_animation_now() {
    unsafe {
        let focus = MC_STATE.with(|s| commit_pending(&mut s.borrow_mut()));
        if !focus.is_null() {
            SetForegroundWindow(focus);
        }
    }
}

/// Flips the cached animation preference immediately, without waiting for
/// the next `show_mission_control` to re-read `Config`. Batch 3 calls this
/// from the config-reload path.
pub fn set_animations_enabled(on: bool) {
    MC_STATE.with(|s| {
        s.borrow_mut().animations_enabled = on;
    });
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
    }

    let scale = (dpi as f32 / 96.0).max(1.0);
    mc.scale = scale;

    let px = |pt: i32| (pt as f32 * scale).round() as i32;

    mc.h_font_title = create_segoe_font(px(20), 700); // Spaces Card Title
    mc.h_font_card = create_segoe_font(px(15), 600); // Window Card Title
    mc.h_font_small = create_segoe_font(px(13), 500); // Spaces Card Subtitle
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

        let pending_focus = MC_STATE.with(|s| {
            let mut mc = s.borrow_mut();
            let pending_focus = commit_pending(&mut mc);
            if mc.is_visible {
                return pending_focus;
            }

            mc.animations_enabled = app_state.config.mission_control_animations;

            mc.active_mon_idx = mon_idx;
            mc.active_desk_idx = desk_idx;
            mc.space_cards.clear();
            mc.window_cards.clear();
            mc.hovered_space = None;
            mc.hovered_window = None;
            mc.dragging_window = None;
            mc.drag_active = false;

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
                    return pending_focus;
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
            rebuild_cards(
                &mut mc,
                app_state,
                mon_idx,
                desk_idx,
                width,
                height,
                AnimRequest::Open,
            );

            if anim_allowed(&mc, mc.window_cards.len()) {
                let anim = Animation {
                    kind: AnimKind::Open,
                    started: std::time::Instant::now(),
                    duration_ms: MC_ANIM_OPEN_MS,
                    eased: 0.0,
                    outgoing: Vec::new(),
                    push_dx: 0,
                    prev_active: None,
                    pending_focus: null_mut(),
                };
                start_animation(&mut mc, anim);
            }

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
            pending_focus
        });

        if !pending_focus.is_null() {
            SetForegroundWindow(pending_focus);
        }
    }
}

/// What a `rebuild_cards` call should capture as each card's animation start
/// state. `None` leaves anim_from_* equal to the final rects (a zero-length
/// tween). Chosen by the caller before the layout is known; Phase 4 wires
/// `Open`/`Switch` in from show/refresh.
enum AnimRequest {
    None,
    Open,
    Switch { push_dx: i32 },
}

fn offset_rect(r: &RECT, dx: i32, dy: i32) -> RECT {
    RECT {
        left: r.left + dx,
        top: r.top + dy,
        right: r.right + dx,
        bottom: r.bottom + dy,
    }
}

/// Scale `r` toward/away from an explicit centre point (not necessarily its
/// own centre) by `factor`. Used to self-zoom a card and its thumbnail about
/// the SAME point so the nesting invariant (thumb inside card) survives the
/// fallback, exactly as it does for the real captured-rect path.
fn scale_about(r: &RECT, cx: i32, cy: i32, factor: f32) -> RECT {
    RECT {
        left: cx + ((r.left - cx) as f32 * factor).round() as i32,
        top: cy + ((r.top - cy) as f32 * factor).round() as i32,
        right: cx + ((r.right - cx) as f32 * factor).round() as i32,
        bottom: cy + ((r.bottom - cy) as f32 * factor).round() as i32,
    }
}

fn rects_intersect(a: &RECT, b: &RECT) -> bool {
    a.left < b.right && a.right > b.left && a.top < b.bottom && a.bottom > b.top
}

/// Real on-screen rect for `hwnd`, converted to the overlay's client
/// coordinates, or `None` when it is unusable as a genuine "fly in" source:
/// minimized, degenerate width/height, or entirely outside the overlay. A
/// merely partially-offscreen rect is returned as-is — DWM clips the
/// destination to the overlay, so the thumbnail just flies in from the edge.
unsafe fn capture_open_source(hwnd: HWND, origin: (i32, i32), overlay: &RECT) -> Option<RECT> {
    if windows_sys::Win32::UI::WindowsAndMessaging::IsIconic(hwnd) != 0 {
        return None;
    }

    let mut r: RECT = std::mem::zeroed();
    let ok = windows_sys::Win32::Graphics::Dwm::DwmGetWindowAttribute(
        hwnd,
        windows_sys::Win32::Graphics::Dwm::DWMWA_EXTENDED_FRAME_BOUNDS as _,
        &mut r as *mut _ as _,
        std::mem::size_of::<RECT>() as u32,
    ) == 0
        || windows_sys::Win32::UI::WindowsAndMessaging::GetWindowRect(hwnd, &mut r) != 0;
    if !ok {
        return None;
    }

    let client = offset_rect(&r, -origin.0, -origin.1);
    if client.right - client.left <= 0 || client.bottom - client.top <= 0 {
        return None;
    }
    if !rects_intersect(&client, overlay) {
        return None;
    }
    Some(client)
}

/// Derives a card's animation source rects (anim_from_thumb, anim_from_card)
/// from a live capture of `target_hwnd`, falling back to a self-zoom of
/// `card_rect`/`thumb_rect` when the window is minimized, degenerate, or
/// entirely off-overlay — same fallback `capture_open_source` signals with
/// `None`. `card_rect`/`thumb_rect` supply the header/margin offsets used to
/// derive `anim_from_card` from the captured `anim_from_thumb`, exactly as
/// they were laid out. Shared by `rebuild_cards` (Open) and
/// `hide_mission_control_focusing` (Close, which must recapture live rather
/// than trust a possibly-stale or Switch-skewed `anim_from_*`).
unsafe fn capture_anim_source_rects(
    target_hwnd: HWND,
    mc_origin: (i32, i32),
    overlay: &RECT,
    card_rect: &RECT,
    thumb_rect: &RECT,
) -> (RECT, RECT) {
    match capture_open_source(target_hwnd, mc_origin, overlay) {
        Some(src) => {
            let margin_x = thumb_rect.left - card_rect.left;
            let header_h = thumb_rect.top - card_rect.top;
            let bottom_margin = card_rect.bottom - thumb_rect.bottom;
            let fc = RECT {
                left: src.left - margin_x,
                top: src.top - header_h,
                right: src.right + margin_x,
                bottom: src.bottom + bottom_margin,
            };
            (src, fc)
        }
        None => {
            let ccx = (card_rect.left + card_rect.right) / 2;
            let ccy = (card_rect.top + card_rect.bottom) / 2;
            (
                scale_about(thumb_rect, ccx, ccy, MC_ANIM_SELF_ZOOM),
                scale_about(card_rect, ccx, ccy, MC_ANIM_SELF_ZOOM),
            )
        }
    }
}

/// Rebuild the spaces bar and window-card grid (unregistering any existing
/// DWM thumbnails first). Shared by `show_mission_control` and
/// `refresh_mission_control`; assumes the overlay window and fonts exist.
/// `anim` selects each card's animation start state — see `AnimRequest`.
/// Returns the outgoing set for a `Switch`: the leftover cards from the
/// space being left, still holding their live thumbnail handles, whose
/// ownership the caller must either hand to an `Animation` or unregister
/// itself. Always empty for `Open`/`None`.
unsafe fn rebuild_cards(
    mc: &mut MissionControl,
    app_state: &mut crate::AppState,
    mon_idx: usize,
    desk_idx: usize,
    width: i32,
    height: i32,
    anim: AnimRequest,
) -> Vec<WindowCard> {
    // Keep existing registrations keyed by source window: a kept thumbnail
    // never leaves DWM composition, so a refresh glides cards to their new
    // rects instead of blinking them out and back in. Whatever is left over
    // after the grid is rebuilt belongs to windows no longer on this space
    // and is unregistered at the end (or, for a Switch, handed off as the
    // outgoing set instead).
    let mut kept_thumbs: HashMap<HWND, isize> = HashMap::new();
    for card in &mc.window_cards {
        if card.h_thumb != 0 {
            kept_thumbs.insert(card.hwnd, card.h_thumb);
        }
    }

    // Snapshot before the clear so a Switch can recover the full leftover
    // cards (not just their thumbnail handles) after the grid loop below has
    // removed the reused ones from `kept_thumbs`.
    let outgoing_snapshot = if matches!(anim, AnimRequest::Switch { .. }) {
        mc.window_cards.clone()
    } else {
        Vec::new()
    };

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

        // The overlay is WS_POPUP with no non-client area, so its window
        // origin equals its client origin; read it live (not a cached
        // monitor rect) so a reopen on a different monitor still lifts from
        // the right place.
        let mc_origin: (i32, i32) = if matches!(anim, AnimRequest::Open) {
            let mut origin: RECT = std::mem::zeroed();
            windows_sys::Win32::UI::WindowsAndMessaging::GetWindowRect(mc.hwnd, &mut origin);
            (origin.left, origin.top)
        } else {
            (0, 0)
        };
        let overlay_rect = RECT {
            left: 0,
            top: 0,
            right: width,
            bottom: height,
        };

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

            // Containment invariant: anim_from_thumb sits inside anim_from_card
            // exactly as thumb_rect sits inside card_rect, so the component-wise
            // lerp of both pairs stays nested at every t (see the "nesting" unit
            // test) and the frame never exposes a gap around the thumbnail.
            let (anim_from_thumb, anim_from_card) = match anim {
                AnimRequest::None => (thumb_rect, card_rect),
                AnimRequest::Switch { push_dx } => (
                    offset_rect(&thumb_rect, push_dx, 0),
                    offset_rect(&card_rect, push_dx, 0),
                ),
                AnimRequest::Open => capture_anim_source_rects(
                    target_hwnd,
                    mc_origin,
                    &overlay_rect,
                    &card_rect,
                    &thumb_rect,
                ),
            };

            mc.window_cards.push(WindowCard {
                hwnd: target_hwnd,
                h_thumb,
                h_icon,
                card_rect,
                thumb_rect,
                title,
                anim_from_card,
                anim_from_thumb,
                draw_rect: card_rect,
            });
        }
    }

    if matches!(anim, AnimRequest::Switch { .. }) {
        // At this point `kept_thumbs` holds exactly the leftovers: reused
        // handles were removed from it during the grid loop above. Recover
        // the matching cards from the snapshot as the outgoing set, and
        // drop any snapshot card whose handle WAS reused — driving or
        // unregistering it twice would corrupt DWM thumbnail state.
        outgoing_snapshot
            .into_iter()
            .filter(|card| kept_thumbs.contains_key(&card.hwnd))
            .collect()
    } else {
        // Windows no longer on this space keep no registration behind.
        for (_, h_thumb) in kept_thumbs {
            DwmUnregisterThumbnail(h_thumb);
        }
        Vec::new()
    }
}

/// Re-sync an already-visible overlay with the desktop state in place —
/// no hide/show, so switching spaces from inside Mission Control (space-card
/// click, digit keys, global hotkeys) never flashes the overlay. Never
/// animated: the non-animated wrapper used by add_space_on / remove_space_on
/// and drag-drop refreshes.
pub fn refresh_mission_control(app_state: &mut crate::AppState) {
    refresh_mission_control_animated(app_state, false);
}

/// `refresh_mission_control`, optionally animated as a space-switch slide.
/// Even with `animate: true`, the transition only actually runs when the
/// displayed space changes (`prev_desk != desk_idx`) and `anim_allowed` —
/// so a move-window hotkey or a drag-drop refresh that leaves the shown
/// space unchanged stays instant.
pub fn refresh_mission_control_animated(app_state: &mut crate::AppState, animate: bool) {
    unsafe {
        let pending_focus = MC_STATE.with(|s| {
            let mut mc = s.borrow_mut();
            let pending_focus = commit_pending(&mut mc);
            if !mc.is_visible || mc.hwnd.is_null() {
                return pending_focus;
            }
            let mon_idx = mc.active_mon_idx;
            if mon_idx >= app_state.desktop_mgr.monitors.len() {
                return pending_focus;
            }
            let prev_desk = mc.active_desk_idx;
            let desk_idx = app_state.desktop_mgr.monitors[mon_idx].current;
            mc.active_desk_idx = desk_idx;

            let mut client_rect: RECT = std::mem::zeroed();
            GetClientRect(mc.hwnd, &mut client_rect);
            let width = client_rect.right - client_rect.left;
            let height = client_rect.bottom - client_rect.top;

            let do_animate = animate && prev_desk != desk_idx;
            let push_dx = if desk_idx > prev_desk {
                width / 12
            } else {
                -width / 12
            };

            let outgoing = rebuild_cards(
                &mut mc,
                app_state,
                mon_idx,
                desk_idx,
                width,
                height,
                if do_animate {
                    AnimRequest::Switch { push_dx }
                } else {
                    AnimRequest::None
                },
            );

            if do_animate {
                let anim = Animation {
                    kind: AnimKind::Switch,
                    started: std::time::Instant::now(),
                    duration_ms: MC_ANIM_SWITCH_MS,
                    eased: 0.0,
                    outgoing,
                    push_dx,
                    prev_active: Some(prev_desk),
                    pending_focus: null_mut(),
                };
                if anim_allowed(&mc, mc.window_cards.len()) {
                    start_animation(&mut mc, anim);
                } else {
                    // Animations off / not allowed: same lifecycle, zero
                    // duration, no timer armed — complete_animation
                    // unregisters the outgoing set that rebuild_cards
                    // already pulled out of `kept_thumbs`, mirroring
                    // hide_mission_control_focusing's fallback below.
                    mc.anim = Some(anim);
                    complete_animation(&mut mc);
                }
            }

            // Card indexes changed; stale hover/drag state must not survive.
            mc.hovered_window = None;
            mc.hovered_close = None;
            mc.hovered_plus = false;
            mc.dragging_window = None;
            mc.drag_active = false;

            // A switch may have activated another window; take the keyboard
            // back so Esc and the digit keys keep working.
            SetForegroundWindow(mc.hwnd);
            InvalidateRect(mc.hwnd, std::ptr::null(), 1);
            pending_focus
        });

        if !pending_focus.is_null() {
            SetForegroundWindow(pending_focus);
        }
    }
}

pub fn hide_mission_control() {
    hide_mission_control_focusing(null_mut());
}

/// `hide_mission_control` with a window to raise once the close finishes —
/// used by the card-click path so focus hand-off survives a deferred close
/// animation. `target` is raised from `complete_animation`'s `pending_focus`,
/// never called here directly (that would be a `SetForegroundWindow` inside
/// the MC_STATE borrow).
pub fn hide_mission_control_focusing(target: HWND) {
    unsafe {
        let pending_focus = MC_STATE.with(|s| {
            let mut mc = s.borrow_mut();
            let mut focus = commit_pending(&mut mc);
            if !mc.is_visible {
                return focus;
            }

            // Honest immediately, so a re-toggle opens fresh and
            // is_mission_control_active() does not lie mid-fade. The actual
            // teardown (thumbnail unregistration, vec clears, SW_HIDE) is
            // deferred into complete_animation.
            mc.is_visible = false;
            mc.hovered_space = None;
            mc.hovered_window = None;
            mc.dragging_window = None;
            mc.drag_active = false;
            mc.hovered_close = None;
            mc.hovered_plus = false;

            // Recapture each card's animation source rect live: anim_from_*
            // was set by the last rebuild_cards call, which for a Switch
            // holds slide-in rects (not real window positions) and for an
            // Open may be stale if the window moved or resized while the
            // overlay was up. A Close always wants where the real window is
            // *now*, so it can shrink back into it.
            let mut origin: RECT = std::mem::zeroed();
            windows_sys::Win32::UI::WindowsAndMessaging::GetWindowRect(mc.hwnd, &mut origin);
            let mc_origin = (origin.left, origin.top);
            let mut client_rect: RECT = std::mem::zeroed();
            GetClientRect(mc.hwnd, &mut client_rect);
            let overlay_rect = RECT {
                left: 0,
                top: 0,
                right: client_rect.right - client_rect.left,
                bottom: client_rect.bottom - client_rect.top,
            };
            for card in &mut mc.window_cards {
                let (anim_from_thumb, anim_from_card) = capture_anim_source_rects(
                    card.hwnd,
                    mc_origin,
                    &overlay_rect,
                    &card.card_rect,
                    &card.thumb_rect,
                );
                card.anim_from_thumb = anim_from_thumb;
                card.anim_from_card = anim_from_card;
            }

            let anim = Animation {
                kind: AnimKind::Close,
                started: std::time::Instant::now(),
                duration_ms: MC_ANIM_CLOSE_MS,
                eased: 0.0,
                outgoing: Vec::new(),
                push_dx: 0,
                prev_active: None,
                pending_focus: target,
            };

            if anim_allowed(&mc, mc.window_cards.len()) {
                start_animation(&mut mc, anim);
            } else {
                // Animations off / not allowed: same lifecycle, zero
                // duration, no timer armed.
                mc.anim = Some(anim);
                focus = complete_animation(&mut mc);
            }

            log_info!("Mission Control hidden");
            focus
        });

        if !pending_focus.is_null() {
            SetForegroundWindow(pending_focus);
        }
    }
}

/// Fail-open probe: SPI_GETCLIENTAREAANIMATION is read fresh at the start of
/// every transition rather than cached, so a live Settings > Accessibility
/// change takes effect on the next open/close/switch without a
/// WM_SETTINGCHANGE handler.
unsafe fn system_animations_enabled() -> bool {
    let mut enabled: BOOL = 1;
    if SystemParametersInfoW(
        SPI_GETCLIENTAREAANIMATION,
        0,
        &mut enabled as *mut BOOL as _,
        0,
    ) == 0
    {
        return true;
    }
    enabled != 0
}

fn anim_allowed(mc: &MissionControl, card_count: usize) -> bool {
    mc.animations_enabled
        && card_count <= MC_ANIM_MAX_CARDS
        && unsafe { system_animations_enabled() }
}

/// Arms the transition timer and computes frame 0 in place, so the first
/// paint after this call is never a tick late.
unsafe fn start_animation(mc: &mut MissionControl, a: Animation) {
    mc.anim = Some(a);
    SetTimer(mc.hwnd, TIMER_MC_ANIM, MC_ANIM_TICK_MS, None);
    tick_animation(mc);
}

/// Disarms the timer, snaps every card to its final rect/opacity via the
/// existing `restore_thumbnail`, runs the deferred Close teardown if that is
/// what was in flight, and RETURNS the focus HWND to raise (or null) instead
/// of calling SetForegroundWindow itself — the caller must be outside the
/// MC_STATE borrow before acting on it.
unsafe fn complete_animation(mc: &mut MissionControl) -> HWND {
    KillTimer(mc.hwnd, TIMER_MC_ANIM);
    let Some(anim) = mc.anim.take() else {
        return null_mut();
    };

    for card in &anim.outgoing {
        if card.h_thumb != 0 {
            DwmUnregisterThumbnail(card.h_thumb);
        }
    }

    for card in &mut mc.window_cards {
        card.draw_rect = card.card_rect;
        restore_thumbnail(card);
    }

    if anim.kind == AnimKind::Close {
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
    }

    anim.pending_focus
}

/// The single re-entrancy rule: every entry point (show, hide, refresh,
/// WM_KEYDOWN, WM_LBUTTONDOWN, WM_LBUTTONUP) calls this FIRST, snapping any
/// in-flight animation to its final state — running a pending Close's
/// teardown if that is what was in flight — before the new request is
/// evaluated. Returns the focus HWND to raise (or null), exactly like
/// `complete_animation`, for the caller to act on outside the MC_STATE
/// borrow.
unsafe fn commit_pending(mc: &mut MissionControl) -> HWND {
    if mc.anim.is_some() {
        complete_animation(mc)
    } else {
        null_mut()
    }
}

/// One WM_TIMER tick: advances `eased`, drives every incoming card's DWM
/// thumbnail and `draw_rect` from that SAME eased value (the next WM_PAINT
/// reads it too), then invalidates the client area. Never calls
/// UpdateWindow / RedrawWindow(RDW_UPDATENOW) here — render_mission_control
/// takes its own MC_STATE.borrow() and would re-enter synchronously against
/// this borrow_mut(), which panics. Returns (done, focus) so the caller can
/// call SetForegroundWindow after releasing the MC_STATE borrow.
unsafe fn tick_animation(mc: &mut MissionControl) -> (bool, HWND) {
    let (kind, eased, duration_ms, started, done) = {
        let Some(anim) = mc.anim.as_mut() else {
            return (true, null_mut());
        };
        let t_raw = (anim.started.elapsed().as_secs_f32() * 1000.0 / anim.duration_ms).min(1.0);
        anim.eased = match anim.kind {
            AnimKind::Close => ease_in_quad(t_raw),
            AnimKind::Open | AnimKind::Switch => ease_out_cubic(t_raw),
        };
        (
            anim.kind,
            anim.eased,
            anim.duration_ms,
            anim.started,
            t_raw >= 1.0,
        )
    };

    // The GDI card fill fakes alpha via colour lerp (Phase 5); this is the
    // one place the DWM thumbnail gets a genuine opacity fade.
    let opacity: u8 = match kind {
        AnimKind::Open => (255.0 * eased).round() as u8,
        AnimKind::Close => (255.0 * (1.0 - eased)).round() as u8,
        AnimKind::Switch => 255,
    };

    for card in &mut mc.window_cards {
        let (card_from, card_to) = anim_endpoints(kind, card.anim_from_card, card.card_rect);
        card.draw_rect = lerp_rect(&card_from, &card_to, eased);
        if card.h_thumb != 0 {
            let mut props: DWM_THUMBNAIL_PROPERTIES = std::mem::zeroed();
            props.dwFlags = DWM_TNP_RECTDESTINATION | DWM_TNP_VISIBLE | DWM_TNP_OPACITY;
            let (thumb_from, thumb_to) =
                anim_endpoints(kind, card.anim_from_thumb, card.thumb_rect);
            props.rcDestination = lerp_rect(&thumb_from, &thumb_to, eased);
            props.fVisible = 1;
            props.opacity = opacity;
            DwmUpdateThumbnailProperties(card.h_thumb, &props);
        }
    }

    // Outgoing (Phase 6, Switch only): pushed the opposite direction of the
    // incoming set's slide-in, fading out as it goes — the switch reads as
    // one layout leaving while the other arrives.
    if let Some(anim) = mc.anim.as_mut() {
        let push_dx = anim.push_dx;
        let dx = lerp_i32(0, -push_dx, eased);
        let out_opacity = (255.0 * (1.0 - eased)).round() as u8;
        for card in &mut anim.outgoing {
            card.draw_rect = offset_rect(&card.card_rect, dx, 0);
            if card.h_thumb != 0 {
                let mut props: DWM_THUMBNAIL_PROPERTIES = std::mem::zeroed();
                props.dwFlags = DWM_TNP_RECTDESTINATION | DWM_TNP_VISIBLE | DWM_TNP_OPACITY;
                props.rcDestination = offset_rect(&card.thumb_rect, dx, 0);
                props.fVisible = 1;
                props.opacity = out_opacity;
                DwmUpdateThumbnailProperties(card.h_thumb, &props);
            }
        }
    }

    InvalidateRect(mc.hwnd, std::ptr::null(), 0);

    if !done {
        return (false, null_mut());
    }

    let wall_ms = started.elapsed().as_secs_f32() * 1000.0;
    if wall_ms > duration_ms * MC_ANIM_OVERRUN_FACTOR {
        let kind_name = match kind {
            AnimKind::Open => "open",
            AnimKind::Close => "close",
            AnimKind::Switch => "switch",
        };
        log_warn!(
            "Mission Control {} animation overran its budget: {:.0}ms wall vs {:.0}ms nominal",
            kind_name,
            wall_ms,
            duration_ms
        );
    }

    (true, complete_animation(mc))
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
            let mut client_rect: RECT = std::mem::zeroed();
            GetClientRect(hwnd, &mut client_rect);
            let width = client_rect.right - client_rect.left;
            let height = client_rect.bottom - client_rect.top;
            let mem_dc = CreateCompatibleDC(hdc);
            let mem_bmp = CreateCompatibleBitmap(hdc, width, height);
            if !mem_dc.is_null() && !mem_bmp.is_null() {
                let old_bmp = SelectObject(mem_dc, mem_bmp as _);
                render_mission_control(mem_dc, hwnd);
                BitBlt(hdc, 0, 0, width, height, mem_dc, 0, 0, SRCCOPY);
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
        WM_TIMER => {
            if wparam == TIMER_MC_ANIM {
                let focus = MC_STATE.with(|s| {
                    let mut mc = s.borrow_mut();
                    tick_animation(&mut mc).1
                });
                if !focus.is_null() {
                    SetForegroundWindow(focus);
                }
            }
            0
        }
        WM_KEYDOWN => {
            // Re-entrancy policy: snap any in-flight animation to its final
            // state before evaluating this keypress.
            let commit_focus = MC_STATE.with(|s| commit_pending(&mut s.borrow_mut()));
            if !commit_focus.is_null() {
                SetForegroundWindow(commit_focus);
            }

            let key = wparam as u32;
            if key == VK_ESCAPE as u32 {
                hide_mission_control();
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
                        refresh_mission_control_animated(state, true);
                    }
                });
            }
            0
        }
        WM_LBUTTONDOWN => {
            let pt = POINT {
                x: (lparam & 0xFFFF) as i16 as i32,
                y: ((lparam >> 16) & 0xFFFF) as i16 as i32,
            };
            let mut action_switch: Option<(usize, usize)> = None;
            let mut action_add: Option<usize> = None;
            let mut action_remove: Option<(usize, usize)> = None;
            let mut should_hide = false;

            let commit_focus = MC_STATE.with(|s| {
                let mut mc = s.borrow_mut();
                // Re-entrancy policy: snap any in-flight animation to its
                // final state before hit-testing this click.
                let commit_focus = commit_pending(&mut mc);

                // The "+" tile lives outside space_cards; test it first.
                if mc.plus_visible && pt_in_rect(&mc.plus_rect, pt) {
                    action_add = Some(mc.active_mon_idx);
                    return commit_focus;
                }

                // Close buttons win over the card body beneath them —
                // otherwise the click would switch to the space instead of
                // removing it. Only offered while more than one space exists.
                if mc.space_cards.len() > 1 {
                    for card in &mc.space_cards {
                        if pt_in_rect(&close_button_rect(&card.rect, mc.scale), pt) {
                            action_remove = Some((mc.active_mon_idx, card.desk_idx));
                            return commit_focus;
                        }
                    }
                }

                // Check spaces bar click. Clicking the already-shown space is
                // a no-op so the overlay doesn't churn its thumbnails.
                for card in &mc.space_cards {
                    if pt_in_rect(&card.rect, pt) {
                        if card.desk_idx != mc.active_desk_idx {
                            action_switch = Some((mc.active_mon_idx, card.desk_idx));
                        }
                        return commit_focus;
                    }
                }

                // Check window card click / drag start
                for (idx, card) in mc.window_cards.iter().enumerate() {
                    if pt_in_rect(&card.card_rect, pt) {
                        mc.dragging_window = Some(idx);
                        mc.drag_active = false;
                        mc.drag_offset = pt;
                        SetCapture(hwnd);
                        return commit_focus;
                    }
                }

                // Clicked backdrop -> dismiss
                should_hide = true;
                commit_focus
            });

            if !commit_focus.is_null() {
                SetForegroundWindow(commit_focus);
            }

            if let Some((mon, desk)) = action_switch {
                // Switch the space underneath but keep Mission Control open,
                // refreshing the overlay in place (no hide/show flash).
                crate::with_app_state(|state| {
                    state.desktop_mgr.switch_desktop(mon, desk, None);
                    refresh_mission_control_animated(state, true);
                });
            } else if let Some(mon) = action_add {
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
                // Hover repaints never fight the tween; clicks are still
                // never swallowed because hit-testing always uses the
                // final `card_rect`, not `draw_rect`.
                if mc.anim.is_some() {
                    return;
                }
                let old_hover_s = mc.hovered_space;
                let old_hover_w = mc.hovered_window;
                let old_hover_plus = mc.hovered_plus;
                let old_hover_close = mc.hovered_close;

                mc.hovered_space = mc.space_cards.iter().position(|c| pt_in_rect(&c.rect, pt));
                mc.hovered_plus = mc.plus_visible && pt_in_rect(&mc.plus_rect, pt);
                mc.hovered_close = if mc.space_cards.len() > 1 {
                    let scale = mc.scale;
                    mc.space_cards
                        .iter()
                        .position(|c| pt_in_rect(&close_button_rect(&c.rect, scale), pt))
                } else {
                    None
                };
                // Window-card hover freezes while a drag is live: the ghost
                // sweeping the grid would otherwise flip the highlight (and
                // repaint) on every card it crosses.
                if !mc.drag_active {
                    mc.hovered_window = mc
                        .window_cards
                        .iter()
                        .position(|c| pt_in_rect(&c.card_rect, pt));
                }

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
            ReleaseCapture();

            let mut move_window_action: Option<(HWND, usize, usize)> = None;
            let mut new_space_action: Option<(HWND, usize)> = None;
            let mut focus_window_action: Option<HWND> = None;

            let commit_focus = MC_STATE.with(|s| {
                let mut mc = s.borrow_mut();
                // Re-entrancy policy: snap any in-flight animation to its
                // final state before evaluating this release.
                let commit_focus = commit_pending(&mut mc);
                if let Some(drag_idx) = mc.dragging_window.take() {
                    let was_drag = mc.drag_active;
                    mc.drag_active = false;
                    let dragged_hwnd = mc.window_cards[drag_idx].hwnd;

                    // Dropped onto the "+" tile -> new space with this window
                    // on it (macOS parity).
                    if mc.plus_visible && pt_in_rect(&mc.plus_rect, pt) {
                        new_space_action = Some((dragged_hwnd, mc.active_mon_idx));
                        return commit_focus;
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
                        return commit_focus;
                    }

                    if was_drag {
                        // Dropped anywhere else: cancel — snap the ghost
                        // thumbnail back into its grid slot.
                        restore_thumbnail(&mc.window_cards[drag_idx]);
                        InvalidateRect(hwnd, std::ptr::null(), 0);
                        return commit_focus;
                    }

                    // Plain click on window card -> Focus Window & Exit!
                    if pt_in_rect(&mc.window_cards[drag_idx].card_rect, pt) {
                        focus_window_action = Some(dragged_hwnd);
                    }
                }
                commit_focus
            });

            if !commit_focus.is_null() {
                SetForegroundWindow(commit_focus);
            }

            if let Some((target_hwnd, mon_idx, target_desk)) = move_window_action {
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
                hide_mission_control_focusing(focus_hwnd);
            }
            0
        }
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

unsafe fn render_mission_control(hdc: windows_sys::Win32::Graphics::Gdi::HDC, hwnd: HWND) {
    MC_STATE.with(|s| {
        let mc = s.borrow();
        // Read once so the blit below and the DWM update that produced this
        // very tick's `draw_rect`/thumbnail rect agree on the same t — see
        // the tearing note on `tick_animation`.
        let anim_t = mc.anim.as_ref().map(|a| (a.kind, a.eased));
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

        let pen_border = CreatePen(PS_SOLID, 1, rgb(0x38, 0x38, 0x42));
        let pen_active = CreatePen(PS_SOLID, px(2), rgb(0x81, 0x8C, 0xF8)); // Indigo Accent Outline
        let pen_drag_target = CreatePen(PS_SOLID, px(2), rgb(0x34, 0xD3, 0x99)); // Emerald Green
        let close_bg = CreateSolidBrush(rgb(0x3A, 0x3A, 0x44));
        let close_bg_hover = CreateSolidBrush(rgb(0xE8, 0x55, 0x5A)); // Destructive Red

        let r_corner = px(12);

        // Space-switch crossfade: the departing active card's tint falls
        // from indigo to plain while the arriving one rises the other way.
        // `card.is_active` already reflects the NEW desk_idx (rebuild_cards
        // ran before the animation started), so only `prev_active` is
        // needed to identify the departing card.
        let switch_eased = match anim_t {
            Some((AnimKind::Switch, eased)) => eased,
            _ => 1.0,
        };
        let switch_rising_bg = CreateSolidBrush(lerp_rgb(
            rgb(0x1F, 0x1F, 0x24),
            rgb(0x28, 0x2A, 0x40),
            switch_eased,
        ));
        let switch_rising_pen = CreatePen(
            PS_SOLID,
            px(2),
            lerp_rgb(rgb(0x38, 0x38, 0x42), rgb(0x81, 0x8C, 0xF8), switch_eased),
        );
        let switch_falling_bg = CreateSolidBrush(lerp_rgb(
            rgb(0x28, 0x2A, 0x40),
            rgb(0x1F, 0x1F, 0x24),
            switch_eased,
        ));
        let switch_falling_pen = CreatePen(
            PS_SOLID,
            px(2),
            lerp_rgb(rgb(0x81, 0x8C, 0xF8), rgb(0x38, 0x38, 0x42), switch_eased),
        );
        let switch_fade = matches!(anim_t, Some((AnimKind::Switch, _)));
        let prev_active_idx = mc.anim.as_ref().and_then(|a| a.prev_active);

        // Open/Close slide the whole bar in/out; Switch leaves it in place
        // and crossfades the active tint instead. Uniform for every space
        // card and the "+" tile — no new fields on SpaceCard.
        let half_card_h = mc
            .space_cards
            .first()
            .map(|c| (c.rect.bottom - c.rect.top) / 2)
            .unwrap_or_else(|| px(50));
        let bar_dy = match anim_t {
            Some((AnimKind::Open, eased)) => lerp_i32(-half_card_h, 0, eased),
            Some((AnimKind::Close, eased)) => lerp_i32(0, -half_card_h, eased),
            _ => 0,
        };

        for (idx, card) in mc.space_cards.iter().enumerate() {
            // Hover/drag decoration is frozen (and pointless to show) while
            // an animation is running — WM_MOUSEMOVE already stops updating
            // it, but a stale value from just before the transition started
            // must not still paint.
            let is_hover = anim_t.is_none() && mc.hovered_space == Some(idx);
            let is_drag_target = anim_t.is_none() && mc.drag_active && is_hover;

            let (brush, pen) = if switch_fade && card.is_active {
                (switch_rising_bg, switch_rising_pen)
            } else if switch_fade && prev_active_idx == Some(idx) {
                (switch_falling_bg, switch_falling_pen)
            } else if anim_t.is_some() {
                if card.is_active {
                    (card_active_bg, pen_active)
                } else {
                    (card_bg, pen_border)
                }
            } else if is_drag_target {
                (card_hover_bg, pen_drag_target)
            } else if card.is_active {
                (card_active_bg, pen_active)
            } else if is_hover {
                (card_hover_bg, pen_border)
            } else {
                (card_bg, pen_border)
            };

            let rect = offset_rect(&card.rect, 0, bar_dy);

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

            // Title: "Space X"
            SelectObject(hdc, mc.h_font_title);
            SetTextColor(hdc, rgb(0xFF, 0xFF, 0xFF));
            let title_text = format!("Space {}", card.desk_idx + 1);
            let mut title_rect = RECT {
                left: rect.left + px(16),
                top: rect.top + px(18),
                right: rect.right - px(16),
                bottom: rect.top + px(48),
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
                left: rect.left + px(16),
                top: rect.top + px(52),
                right: rect.right - px(16),
                bottom: rect.bottom - px(16),
            };
            draw_text_wide(
                hdc,
                &sub_text,
                &mut sub_rect,
                DT_CENTER | DT_SINGLELINE | DT_VCENTER,
            );

            // Close button, macOS style: only on the hovered card, and never
            // when this is the monitor's last space. `is_hover` is already
            // forced false while animating, so this never draws mid-tween.
            if is_hover && mc.space_cards.len() > 1 {
                let cb = close_button_rect(&rect, scale);
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
                SelectObject(hdc, mc.h_font_small);
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

        // The "+" tile: same visual family as the space cards, emerald drag
        // outline when a window drag hovers it (drop = new space + move).
        if mc.plus_visible {
            let is_hover = anim_t.is_none() && mc.hovered_plus;
            let is_drag_target = anim_t.is_none() && mc.drag_active && is_hover;
            let plus_rect = offset_rect(&mc.plus_rect, 0, bar_dy);
            SelectObject(hdc, if is_hover { card_hover_bg } else { card_bg });
            SelectObject(
                hdc,
                if is_drag_target {
                    pen_drag_target
                } else {
                    pen_border
                },
            );
            RoundRect(
                hdc,
                plus_rect.left,
                plus_rect.top,
                plus_rect.right,
                plus_rect.bottom,
                r_corner,
                r_corner,
            );
            SelectObject(hdc, mc.h_font_title);
            SetTextColor(
                hdc,
                if is_hover {
                    rgb(0xFF, 0xFF, 0xFF)
                } else {
                    rgb(0x9C, 0x9C, 0xA4)
                },
            );
            let mut plus_text_rect = plus_rect;
            draw_text_wide(
                hdc,
                "+",
                &mut plus_text_rect,
                DT_CENTER | DT_SINGLELINE | DT_VCENTER,
            );
        }

        DeleteObject(card_bg);
        DeleteObject(card_active_bg);
        DeleteObject(card_hover_bg);
        DeleteObject(pen_border);
        DeleteObject(pen_active);
        DeleteObject(pen_drag_target);
        DeleteObject(close_bg);
        DeleteObject(close_bg_hover);
        DeleteObject(switch_rising_bg);
        DeleteObject(switch_rising_pen);
        DeleteObject(switch_falling_bg);
        DeleteObject(switch_falling_pen);

        // 3. Render Window Cards (Exposé Grid)
        // A Switch onto an empty space must still let the outgoing set
        // finish its slide-out, so the empty-space shortcut only applies
        // when there is nothing left to animate away either.
        let has_outgoing = mc.anim.as_ref().is_some_and(|a| !a.outgoing.is_empty());
        if mc.window_cards.is_empty() && !has_outgoing {
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
        let header_h = px(38);

        // The only way to fake alpha in GDI: lerp the fill/border between
        // the backdrop colour and the card's resting colour by the kind's
        // fade factor. Open fades in from the backdrop, Close fades out to
        // it, Switch stays fully opaque (geometry-only move, matching
        // tick_animation's thumbnail opacity rule).
        let anim_fade = match anim_t {
            Some((AnimKind::Open, eased)) => Some(eased),
            Some((AnimKind::Close, eased)) => Some(1.0 - eased),
            Some((AnimKind::Switch, _)) => Some(1.0),
            None => None,
        };
        let (anim_fill_brush, anim_border_pen) = match anim_fade {
            Some(fade) => (
                CreateSolidBrush(lerp_rgb(rgb(0x14, 0x14, 0x18), rgb(0x1F, 0x1F, 0x24), fade)),
                CreatePen(
                    PS_SOLID,
                    1,
                    lerp_rgb(rgb(0x14, 0x14, 0x18), rgb(0x38, 0x38, 0x42), fade),
                ),
            ),
            None => (null_mut(), null_mut()),
        };

        // Outgoing (Phase 6, Switch only): drawn FIRST so the incoming set
        // painted below sits on top of it. Reuses the same anim_fill_brush /
        // anim_border_pen as the incoming cards — for Switch that is the
        // fully opaque resting colour; the actual fade lives in the DWM
        // thumbnail's opacity, driven by tick_animation. No icon, title or
        // hover decoration, matching the incoming cards' animated path.
        if let Some(anim) = mc.anim.as_ref() {
            for card in &anim.outgoing {
                let rect = card.draw_rect;
                SelectObject(hdc, anim_fill_brush);
                SelectObject(hdc, anim_border_pen);
                RoundRect(
                    hdc,
                    rect.left,
                    rect.top,
                    rect.right,
                    rect.bottom,
                    card_corner,
                    card_corner,
                );
            }
        }

        for (idx, card) in mc.window_cards.iter().enumerate() {
            // `draw_rect` equals `card_rect` at rest (rebuild_cards and
            // complete_animation both guarantee it), so this is the final
            // geometry whether or not a transition is running.
            let rect = card.draw_rect;

            if anim_fade.is_some() {
                // Unreadable during a 160-200 ms move, and DrawIconEx +
                // DrawTextW are the two most expensive per-card GDI calls —
                // skip both, along with all hover/drag decoration.
                SelectObject(hdc, anim_fill_brush);
                SelectObject(hdc, anim_border_pen);
                RoundRect(
                    hdc,
                    rect.left,
                    rect.top,
                    rect.right,
                    rect.bottom,
                    card_corner,
                    card_corner,
                );
                continue;
            }

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
                rect.left,
                rect.top,
                rect.right,
                rect.bottom,
                card_corner,
                card_corner,
            );

            // Draw Real Window Icon if available
            let icon_x = rect.left + px(12);
            let icon_y = rect.top + (header_h - icon_size) / 2;
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
                rect.left + px(14)
            };

            let mut title_r = RECT {
                left: text_left,
                top: rect.top,
                right: rect.right - px(14),
                bottom: rect.top + header_h,
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
        DeleteObject(anim_fill_brush);
        DeleteObject(anim_border_pen);
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
    let plus_w = px(56);
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
        assert_eq!(rect_w(&m.plus_rect), 56);
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
    fn lerp_rect_endpoints_return_inputs() {
        let a = RECT {
            left: 0,
            top: 0,
            right: 100,
            bottom: 50,
        };
        let b = RECT {
            left: 200,
            top: 80,
            right: 400,
            bottom: 250,
        };
        let r0 = lerp_rect(&a, &b, 0.0);
        assert_eq!(r0.left, a.left);
        assert_eq!(r0.top, a.top);
        assert_eq!(r0.right, a.right);
        assert_eq!(r0.bottom, a.bottom);
        let r1 = lerp_rect(&a, &b, 1.0);
        assert_eq!(r1.left, b.left);
        assert_eq!(r1.top, b.top);
        assert_eq!(r1.right, b.right);
        assert_eq!(r1.bottom, b.bottom);
    }

    #[test]
    fn lerp_rgb_channels_are_independent() {
        let a = rgb(10, 20, 30);

        // Only red differs between a and b: green/blue must pass through unchanged.
        let b_r = rgb(110, 20, 30);
        let mid_r = lerp_rgb(a, b_r, 0.5);
        assert_eq!(mid_r & 0xFF, 60);
        assert_eq!((mid_r >> 8) & 0xFF, 20);
        assert_eq!((mid_r >> 16) & 0xFF, 30);

        // Only green differs.
        let b_g = rgb(10, 120, 30);
        let mid_g = lerp_rgb(a, b_g, 0.5);
        assert_eq!(mid_g & 0xFF, 10);
        assert_eq!((mid_g >> 8) & 0xFF, 70);
        assert_eq!((mid_g >> 16) & 0xFF, 30);

        // Only blue differs.
        let b_b = rgb(10, 20, 130);
        let mid_b = lerp_rgb(a, b_b, 0.5);
        assert_eq!(mid_b & 0xFF, 10);
        assert_eq!((mid_b >> 8) & 0xFF, 20);
        assert_eq!((mid_b >> 16) & 0xFF, 80);
    }

    #[test]
    fn lerp_rect_preserves_nesting() {
        // A card-sized outer rect and a thumbnail-sized inner rect, nested at
        // both animation endpoints — mirrors card_rect/thumb_rect.
        let outer_a = RECT {
            left: 0,
            top: 0,
            right: 300,
            bottom: 200,
        };
        let inner_a = RECT {
            left: 20,
            top: 20,
            right: 280,
            bottom: 180,
        };
        let outer_b = RECT {
            left: 500,
            top: 300,
            right: 900,
            bottom: 700,
        };
        let inner_b = RECT {
            left: 540,
            top: 340,
            right: 860,
            bottom: 660,
        };

        for &t in &[0.0f32, 0.25, 0.5, 0.75, 1.0] {
            let outer = lerp_rect(&outer_a, &outer_b, t);
            let inner = lerp_rect(&inner_a, &inner_b, t);
            assert!(inner.left >= outer.left, "t={t}");
            assert!(inner.top >= outer.top, "t={t}");
            assert!(inner.right <= outer.right, "t={t}");
            assert!(inner.bottom <= outer.bottom, "t={t}");
        }
    }

    #[test]
    fn anim_endpoints_close_runs_opposite_open_and_switch() {
        // anim_from stands in for the captured source rect, final_rect for
        // the grid rect a card lays out to.
        let anim_from = RECT {
            left: 10,
            top: 10,
            right: 110,
            bottom: 60,
        };
        let final_rect = RECT {
            left: 200,
            top: 150,
            right: 400,
            bottom: 350,
        };

        // Open and Switch are entrances: they start at the source and arrive
        // at the grid rect.
        for kind in [AnimKind::Open, AnimKind::Switch] {
            let (from, to) = anim_endpoints(kind, anim_from, final_rect);
            assert_eq!(from.left, anim_from.left);
            assert_eq!(from.top, anim_from.top);
            assert_eq!(from.right, anim_from.right);
            assert_eq!(from.bottom, anim_from.bottom);
            assert_eq!(to.left, final_rect.left);
            assert_eq!(to.top, final_rect.top);
            assert_eq!(to.right, final_rect.right);
            assert_eq!(to.bottom, final_rect.bottom);
        }

        // Close is an exit: it starts where the card already sits (the grid
        // rect) and shrinks back into the captured source rect — the
        // opposite direction of Open/Switch.
        let (from, to) = anim_endpoints(AnimKind::Close, anim_from, final_rect);
        assert_eq!(from.left, final_rect.left);
        assert_eq!(from.top, final_rect.top);
        assert_eq!(from.right, final_rect.right);
        assert_eq!(from.bottom, final_rect.bottom);
        assert_eq!(to.left, anim_from.left);
        assert_eq!(to.top, anim_from.top);
        assert_eq!(to.right, anim_from.right);
        assert_eq!(to.bottom, anim_from.bottom);
    }
}

unsafe fn create_segoe_font(height: i32, weight: i32) -> HFONT {
    let font_name: Vec<u16> = "Segoe UI Variable Display\0".encode_utf16().collect();
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
