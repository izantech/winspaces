//! Full-screen Exposé-style overlay: a spaces bar of live space thumbnails
//! plus a window-card grid for the active space, with drag-and-drop between
//! them. State and lifecycle live here; see the sibling modules for the pure
//! geometry, DWM thumbnail bookkeeping, painting, and input handling.

mod cards;
mod geometry;
mod input;
mod render;

pub use geometry::neighbor_slot;

use std::cell::RefCell;
use std::ptr::null_mut;
use std::sync::OnceLock;
use windows_sys::Win32::Foundation::{HWND, POINT, RECT};
use windows_sys::Win32::Graphics::Dwm::DwmUnregisterThumbnail;
use windows_sys::Win32::Graphics::Gdi::{InvalidateRect, HFONT};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, GetClientRect, SetForegroundWindow, SetWindowPos, ShowWindow,
    SWP_FRAMECHANGED, SWP_NOACTIVATE, SWP_NOZORDER, SW_HIDE, SW_SHOW, WS_EX_TOOLWINDOW,
    WS_EX_TOPMOST, WS_POPUP,
};
use winspaces_common::log_info;
use winspaces_core::spaces::SpaceManager;
use winspaces_win32::display;
use winspaces_win32::dpi;
use winspaces_win32::dwm;
use winspaces_win32::module::app_instance;
use winspaces_win32::text::encode_wide;
use winspaces_win32::window_class::register_class;

#[allow(clippy::upper_case_acronyms)]
pub type HICON = *mut std::ffi::c_void;

const MC_CLASS_NAME: &str = "WinSpacesMissionControl";

/// One-shot timer that flushes the final frame of a throttled space drag.
const TIMER_DRAG_PAINT: usize = 1;

/// Actions Mission Control cannot perform itself because they coordinate state
/// this module must not see (the daemon's application state: config, hotkeys,
/// tray badge, persisted layouts). The host installs this once at startup.
///
/// Fn pointers, deliberately, not posted messages: `WM_LBUTTONUP` completes a
/// reorder *and* the overlay refresh synchronously before it returns. Deferring
/// either to the next message-loop turn would change the frame in which the
/// overlay repaints after a drop.
///
/// Every entry is expected to take the host's state borrow at the instant it is
/// called and release it before returning — the same borrow window the inline
/// code had before the indirection, so the drag state machine's re-entrancy
/// behaviour is unchanged.
pub struct McHost {
    pub add_space: fn(mon: usize),
    pub remove_space: fn(mon: usize, space: usize),
    pub reorder_space: fn(mon: usize, from: usize, to: usize),
    /// Move the monitor's *active* space one slot in `delta`'s direction. A
    /// separate entry from `reorder_space` because the source slot is the live
    /// `current`, which only the host can read.
    pub reorder_space_neighbor: fn(mon: usize, delta: i32),
    pub switch_space: fn(mon: usize, space: usize),
    pub move_window_to_space: fn(hwnd: HWND, mon: usize, space: usize),
    pub move_window_to_new_space: fn(hwnd: HWND, mon: usize),
    pub close_window: fn(hwnd: HWND),
    pub toggle_window_sticky: fn(hwnd: HWND),
}

static HOST: OnceLock<&'static McHost> = OnceLock::new();

/// Install the host vtable. First call wins; later calls are ignored.
pub fn install_host(host: &'static McHost) {
    let _ = HOST.set(host);
}

/// The installed host, or `None` before startup finished wiring it up. Every
/// caller is a user gesture on a visible overlay, so `None` cannot happen in
/// practice — it is a no-op rather than a panic all the same.
fn host() -> Option<&'static McHost> {
    HOST.get().copied()
}

#[derive(Clone)]
pub struct SpaceCard {
    pub space_idx: usize,
    pub rect: RECT,
    pub window_count: usize,
    pub is_active: bool,
    pub is_tiled: bool,
}

#[derive(Clone)]
pub struct WindowCard {
    pub hwnd: HWND,
    pub h_thumb: isize,
    pub h_icon: HICON,
    pub card_rect: RECT,
    pub thumb_rect: RECT,
    pub title: String,
    pub is_sticky: bool,
}

pub struct MissionControl {
    pub hwnd: HWND,
    pub is_visible: bool,
    pub active_mon_idx: usize,
    pub active_space_idx: usize,
    pub scale: f32,
    pub space_cards: Vec<SpaceCard>,
    pub window_cards: Vec<WindowCard>,
    /// The "+" tile sits *outside* `space_cards` on purpose: every consumer of
    /// that vector (click-switch, drag-drop, digit render) may then assume it
    /// contains real spaces only.
    pub plus_rect: RECT,
    pub plus_visible: bool,
    /// Measured width of the "New Space" label in `h_font_small`, device
    /// pixels, so the tile fits the current language. Refreshed with the
    /// fonts; drag-time relayouts in `input.rs` reuse it.
    pub plus_label_w: i32,
    pub hovered_plus: bool,
    pub hovered_space: Option<usize>,
    /// Space card whose close button the pointer is over. Distinct from
    /// `hovered_space`: the button sits inside the card, and clicking it must
    /// remove the space rather than switch to it.
    pub hovered_close: Option<usize>,
    pub hovered_window: Option<usize>,
    /// Window card whose close button the pointer is over.
    pub hovered_window_close: Option<usize>,
    /// Window card whose pin/sticky button the pointer is over.
    pub hovered_window_pin: Option<usize>,
    /// Window card whose close button the pointer went *down* on. Closing an
    /// app is not undoable, so — unlike the space close button — the action
    /// waits for the button-up over the same target, leaving a misclick a way
    /// out by sliding off the circle before releasing.
    pub pressed_window_close: Option<usize>,
    /// Window card whose pin button the pointer went *down* on.
    pub pressed_window_pin: Option<usize>,
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
    /// Segoe Fluent Icons, for the window pin/sticky button glyph.
    pub h_font_pin: HFONT,
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
            active_space_idx: 0,
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
            plus_label_w: 0,
            hovered_plus: false,
            hovered_space: None,
            hovered_close: None,
            hovered_window: None,
            hovered_window_close: None,
            hovered_window_pin: None,
            pressed_window_close: None,
            pressed_window_pin: None,
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
            h_font_pin: null_mut(),
        }
    }
}

impl Default for MissionControl {
    fn default() -> Self {
        Self::new()
    }
}

pub fn init_mission_control() {
    log_info!("Initialized Mission Control subsystem");
}

pub fn is_mission_control_active() -> bool {
    MC_STATE.with(|s| s.borrow().is_visible)
}

pub fn toggle_mission_control(mgr: &mut SpaceManager) {
    if is_mission_control_active() {
        hide_mission_control();
    } else {
        show_mission_control(mgr);
    }
}

pub fn show_mission_control(mgr: &mut SpaceManager) {
    mgr.scan_untracked_windows();
    unsafe {
        // 1. Determine active monitor & active space
        let mon_idx = mgr.get_active_monitor_index();
        if mon_idx >= mgr.monitors.len() {
            return;
        }
        let space_idx = mgr.monitors[mon_idx].current;
        let hmon = mgr.monitors[mon_idx].hmon;

        let Some(mon_rect) = display::monitor_rect_of(hmon) else {
            return;
        };
        let width = mon_rect.right - mon_rect.left;
        let height = mon_rect.bottom - mon_rect.top;

        MC_STATE.with(|s| {
            let mut mc = s.borrow_mut();
            if mc.is_visible {
                return;
            }

            mc.active_mon_idx = mon_idx;
            mc.active_space_idx = space_idx;
            mc.space_cards.clear();
            mc.window_cards.clear();
            mc.hovered_space = None;
            mc.hovered_window = None;
            mc.hovered_window_close = None;
            mc.pressed_window_close = None;
            mc.dragging_window = None;
            mc.drag_active = false;
            mc.dragging_space = None;
            mc.drag_space_active = false;
            mc.drag_space_current_x = 0;
            mc.drag_space_target_slot = None;

            // 2. Ensure Window Class & HWND
            if mc.hwnd.is_null() {
                register_class(MC_CLASS_NAME, Some(input::mc_wnd_proc));
                let hinst = app_instance();
                let class_name = encode_wide(MC_CLASS_NAME);

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

                dwm::set_dark_mode(mc.hwnd, true);
                dwm::set_backdrop(mc.hwnd, dwm::Backdrop::Acrylic);
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
            let scale = dpi::scale_for_window(mc.hwnd);
            cards::update_fonts_for_dpi(&mut mc, scale);

            // 4/5. Build spaces bar + window grid + thumbnails
            cards::rebuild_cards(&mut mc, mgr, mon_idx, space_idx, width, height);

            mc.is_visible = true;
            ShowWindow(mc.hwnd, SW_SHOW);
            SetForegroundWindow(mc.hwnd);
            InvalidateRect(mc.hwnd, std::ptr::null(), 1);
            log_info!(
                "Mission Control shown on Mon {} (Space {}) with {} window thumbnails",
                mon_idx + 1,
                space_idx + 1,
                mc.window_cards.len()
            );
        });
    }
}

/// Re-sync an already-visible overlay with the live space state in place —
/// no hide/show, so switching spaces from inside Mission Control (space-card
/// click, digit keys, global hotkeys) never flashes the overlay.
pub fn refresh_mission_control(mgr: &mut SpaceManager) {
    unsafe {
        MC_STATE.with(|s| {
            let mut mc = s.borrow_mut();
            if !mc.is_visible || mc.hwnd.is_null() {
                return;
            }
            let mon_idx = mc.active_mon_idx;
            if mon_idx >= mgr.monitors.len() {
                return;
            }
            let space_idx = mgr.monitors[mon_idx].current;
            mc.active_space_idx = space_idx;

            let mut client_rect: RECT = std::mem::zeroed();
            GetClientRect(mc.hwnd, &mut client_rect);
            let width = client_rect.right - client_rect.left;
            let height = client_rect.bottom - client_rect.top;

            cards::rebuild_cards(&mut mc, mgr, mon_idx, space_idx, width, height);

            // Card indexes changed; stale hover/drag/press state must not
            // survive — an armed close button would resolve to whichever
            // window inherited that slot.
            mc.pressed_window_close = None;
            mc.dragging_window = None;
            mc.drag_active = false;
            mc.dragging_space = None;
            mc.drag_space_active = false;
            mc.drag_space_current_x = 0;
            mc.drag_space_target_slot = None;
            // Hover is re-derived rather than cleared: the pointer has not
            // moved, so leaving it blank would drop the highlight off the card
            // under the cursor until the user jiggles the mouse.
            input::resync_hover(&mut mc);

            // A switch may have activated another window; take the keyboard
            // back so Esc and the digit keys keep working.
            SetForegroundWindow(mc.hwnd);
            InvalidateRect(mc.hwnd, std::ptr::null(), 1);
        });
    }
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

            // Every show rebuilds the fonts for the target DPI, so keeping
            // them across the close retained five GDI handles for nothing.
            cards::release_fonts(&mut mc);

            mc.is_visible = false;
            mc.dragging_window = None;
            mc.drag_active = false;
            mc.dragging_space = None;
            mc.drag_space_active = false;
            mc.drag_space_current_x = 0;
            mc.drag_space_target_slot = None;
            mc.hovered_close = None;
            mc.hovered_window_close = None;
            mc.pressed_window_close = None;
            mc.hovered_plus = false;
            log_info!("Mission Control hidden");
        });
    }
}
