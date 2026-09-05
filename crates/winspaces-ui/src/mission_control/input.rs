//! The overlay's wndproc: mouse/keyboard interaction, space-card and
//! window-card drag lifecycles, and the hover state they all read.

use super::geometry::{
    close_button_rect, pt_in_rect, spaces_bar_metrics, spaces_bar_strip_rect,
    target_slot_for_center, window_close_button_rect, window_pin_button_rect,
};
use super::render::render_mission_control;
use super::{host, MissionControl, WindowCard, MC_STATE, TIMER_DRAG_PAINT};
use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows_sys::Win32::Graphics::Dwm::{
    DwmUpdateThumbnailProperties, DWM_THUMBNAIL_PROPERTIES, DWM_TNP_OPACITY,
    DWM_TNP_RECTDESTINATION, DWM_TNP_VISIBLE,
};
use windows_sys::Win32::Graphics::Gdi::{
    BeginPaint, EndPaint, InvalidateRect, ScreenToClient, PAINTSTRUCT,
};
use windows_sys::Win32::System::SystemInformation::GetTickCount;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    GetKeyState, ReleaseCapture, SetCapture, VK_CONTROL, VK_ESCAPE, VK_LEFT, VK_NUMPAD1,
    VK_NUMPAD9, VK_RIGHT, VK_SHIFT,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    DefWindowProcW, GetClientRect, GetCursorPos, GetSystemMetrics, KillTimer, SetForegroundWindow,
    SetTimer, SM_CXDRAG, SM_CYDRAG, WM_CAPTURECHANGED, WM_ERASEBKGND, WM_KEYDOWN, WM_LBUTTONDOWN,
    WM_LBUTTONUP, WM_MOUSEMOVE, WM_PAINT, WM_TIMER,
};
use winspaces_common::log_info;
use winspaces_win32::display;
use winspaces_win32::gdi::surface::double_buffer;

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
        mc.hovered_window_close = mc
            .window_cards
            .iter()
            .position(|c| pt_in_rect(&window_close_button_rect(&c.card_rect, scale), pt));
        mc.hovered_window_pin = mc
            .window_cards
            .iter()
            .position(|c| pt_in_rect(&window_pin_button_rect(&c.card_rect, scale), pt));
    } else {
        mc.hovered_window_close = None;
        mc.hovered_window_pin = None;
    }
}

/// Same, for the pointer wherever it currently sits — used after a rebuild,
/// which moves the cards out from under a cursor that never moved.
pub(crate) unsafe fn resync_hover(mc: &mut MissionControl) {
    let mut pt: POINT = std::mem::zeroed();
    if GetCursorPos(&mut pt) == 0 || ScreenToClient(mc.hwnd, &mut pt) == 0 {
        mc.hovered_space = None;
        mc.hovered_window = None;
        mc.hovered_window_close = None;
        mc.hovered_window_pin = None;
        mc.hovered_plus = false;
        mc.hovered_close = None;
        return;
    }
    hover_at(mc, pt);
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

/// Client-area point packed in a mouse message's `lParam`.
fn client_point(lparam: LPARAM) -> POINT {
    POINT {
        x: (lparam & 0xFFFF) as i16 as i32,
        y: ((lparam >> 16) & 0xFFFF) as i16 as i32,
    }
}

/// What a button press landed on, in the order the overlay tests them:
/// the "+" tile first (it lives outside `space_cards`), then the buttons
/// that sit on top of cards, then the cards themselves.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum PressTarget {
    Plus,
    /// A space card's close button, by card index. Only offered while more
    /// than one space exists.
    SpaceClose(usize),
    /// A window card's pin button, by card index.
    WindowPin(usize),
    /// A window card's close button, by card index.
    WindowClose(usize),
    SpaceCard(usize),
    WindowCard(usize),
    Backdrop,
}

pub(super) fn press_target(mc: &MissionControl, pt: POINT) -> PressTarget {
    let scale = mc.scale;
    if mc.plus_visible && pt_in_rect(&mc.plus_rect, pt) {
        return PressTarget::Plus;
    }
    // Close buttons win over the space card body beneath them; otherwise
    // the click would switch to the space instead of removing it.
    if mc.space_cards.len() > 1 {
        if let Some(idx) = mc
            .space_cards
            .iter()
            .position(|c| pt_in_rect(&close_button_rect(&c.rect, scale), pt))
        {
            return PressTarget::SpaceClose(idx);
        }
    }
    if let Some(idx) = mc
        .window_cards
        .iter()
        .position(|c| pt_in_rect(&window_pin_button_rect(&c.card_rect, scale), pt))
    {
        return PressTarget::WindowPin(idx);
    }
    if let Some(idx) = mc
        .window_cards
        .iter()
        .position(|c| pt_in_rect(&window_close_button_rect(&c.card_rect, scale), pt))
    {
        return PressTarget::WindowClose(idx);
    }
    if let Some(idx) = mc.space_cards.iter().position(|c| pt_in_rect(&c.rect, pt)) {
        return PressTarget::SpaceCard(idx);
    }
    if let Some(idx) = mc
        .window_cards
        .iter()
        .position(|c| pt_in_rect(&c.card_rect, pt))
    {
        return PressTarget::WindowCard(idx);
    }
    PressTarget::Backdrop
}

/// What a button-down asks of the host once the `MC_STATE` borrow is gone.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum DownIntent {
    None,
    AddSpace(usize),
    RemoveSpace(usize, usize),
    Hide,
}

/// Button-down: arm buttons and start drags (those need capture, which is
/// taken while the state is borrowed so `WM_CAPTURECHANGED` can tell a live
/// borrow from a cancelled drag); space add/remove and the backdrop dismiss
/// are handed back as intents.
///
/// # Safety
/// `hwnd` is the overlay window; called from its wndproc.
unsafe fn on_lbuttondown(mc: &mut MissionControl, hwnd: HWND, pt: POINT) -> DownIntent {
    match press_target(mc, pt) {
        PressTarget::Plus => DownIntent::AddSpace(mc.active_mon_idx),
        PressTarget::SpaceClose(idx) => {
            DownIntent::RemoveSpace(mc.active_mon_idx, mc.space_cards[idx].space_idx)
        }
        // Window card buttons are only *armed* here: the action itself waits
        // for the matching button-up (see `on_lbuttonup`), and capture keeps
        // that up arriving even if the pointer leaves the circle first.
        PressTarget::WindowPin(idx) => {
            mc.pressed_window_pin = Some(idx);
            SetCapture(hwnd);
            DownIntent::None
        }
        PressTarget::WindowClose(idx) => {
            mc.pressed_window_close = Some(idx);
            SetCapture(hwnd);
            DownIntent::None
        }
        PressTarget::SpaceCard(idx) => {
            mc.dragging_space = Some(idx);
            mc.drag_space_active = false;
            mc.drag_offset = pt;
            mc.drag_space_current_x = pt.x;
            mc.drag_space_target_slot = Some(idx);
            SetCapture(hwnd);
            DownIntent::None
        }
        PressTarget::WindowCard(idx) => {
            mc.dragging_window = Some(idx);
            mc.drag_active = false;
            mc.drag_offset = pt;
            SetCapture(hwnd);
            DownIntent::None
        }
        PressTarget::Backdrop => DownIntent::Hide,
    }
}

/// What a button-up asks of the host once the `MC_STATE` borrow is gone.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum UpIntent {
    None,
    ToggleSticky(HWND),
    CloseWindow(HWND),
    ReorderSpace {
        mon: usize,
        from: usize,
        to: usize,
    },
    SwitchSpace {
        mon: usize,
        space: usize,
    },
    MoveWindow {
        hwnd: HWND,
        mon: usize,
        space: usize,
    },
    NewSpaceWith {
        hwnd: HWND,
        mon: usize,
    },
    FocusWindow(HWND),
}

/// Button-up: resolve armed buttons and finish drags. Every drag state is
/// taken here, before the caller releases capture.
///
/// # Safety
/// `hwnd` is the overlay window; called from its wndproc.
unsafe fn on_lbuttonup(mc: &mut MissionControl, hwnd: HWND, pt: POINT) -> UpIntent {
    // An armed pin button fires only if released over the pin button.
    if let Some(idx) = mc.pressed_window_pin.take() {
        let scale = mc.scale;
        return match mc.window_cards.get(idx) {
            Some(card) if pt_in_rect(&window_pin_button_rect(&card.card_rect, scale), pt) => {
                UpIntent::ToggleSticky(card.hwnd)
            }
            _ => UpIntent::None,
        };
    }

    // An armed close button fires only if the pointer is still on the same
    // circle; released anywhere else it is a cancelled misclick, and must
    // not fall through to the card underneath (which would focus the window
    // it just declined to close).
    if let Some(idx) = mc.pressed_window_close.take() {
        let scale = mc.scale;
        return match mc.window_cards.get(idx) {
            Some(card) if pt_in_rect(&window_close_button_rect(&card.card_rect, scale), pt) => {
                UpIntent::CloseWindow(card.hwnd)
            }
            _ => UpIntent::None,
        };
    }

    // Space card drag-and-drop or click-to-switch.
    if let Some(drag_s_idx) = mc.dragging_space.take() {
        let was_drag = mc.drag_space_active;
        mc.drag_space_active = false;
        let target_slot = mc.drag_space_target_slot.take().unwrap_or(drag_s_idx);
        let Some(card_space) = mc.space_cards.get(drag_s_idx).map(|c| c.space_idx) else {
            return UpIntent::None;
        };

        if was_drag {
            // Only x decides the drop: the card is pinned to the bar while
            // dragging, but the pointer is free to roam anywhere over the
            // overlay without losing the move.
            if drag_s_idx != target_slot {
                // `space_cards` mirrors `mon.spaces` one for one (see
                // `rebuild_cards`), so the card's slot *is* its space index:
                // both arguments of `reorder_space` live in the same domain.
                return UpIntent::ReorderSpace {
                    mon: mc.active_mon_idx,
                    from: drag_s_idx,
                    to: target_slot,
                };
            }
            InvalidateRect(hwnd, std::ptr::null(), 0);
            return UpIntent::None;
        }

        // Plain click on a space card: switch to it if it is not active.
        if card_space != mc.active_space_idx {
            return UpIntent::SwitchSpace {
                mon: mc.active_mon_idx,
                space: card_space,
            };
        }
        return UpIntent::None;
    }

    if let Some(drag_idx) = mc.dragging_window.take() {
        let was_drag = mc.drag_active;
        mc.drag_active = false;
        let dragged_hwnd = mc.window_cards[drag_idx].hwnd;

        // Dropped onto the "+" tile: a new space with this window on it
        // (macOS parity).
        if mc.plus_visible && pt_in_rect(&mc.plus_rect, pt) {
            return UpIntent::NewSpaceWith {
                hwnd: dragged_hwnd,
                mon: mc.active_mon_idx,
            };
        }

        // Dropped onto a space card: move the window there. The target is
        // the card's space_idx, not its position in the vector; those agree
        // only while the vector is exactly the spaces in order.
        if let Some(target_space) = mc
            .space_cards
            .iter()
            .find(|c| pt_in_rect(&c.rect, pt))
            .map(|c| c.space_idx)
        {
            return UpIntent::MoveWindow {
                hwnd: dragged_hwnd,
                mon: mc.active_mon_idx,
                space: target_space,
            };
        }

        if was_drag {
            // Dropped anywhere else: cancel, snapping the ghost thumbnail
            // back into its grid slot.
            restore_thumbnail(&mc.window_cards[drag_idx]);
            InvalidateRect(hwnd, std::ptr::null(), 0);
            return UpIntent::None;
        }

        // Plain click on a window card: focus it and leave.
        if pt_in_rect(&mc.window_cards[drag_idx].card_rect, pt) {
            return UpIntent::FocusWindow(dragged_hwnd);
        }
    }
    UpIntent::None
}

/// Pointer motion: advance a space-card drag (throttled to the display's
/// frame rate), lift a window drag past the system threshold, and repaint
/// only the cards whose hover state changed.
///
/// # Safety
/// `hwnd` is the overlay window; called from its wndproc.
unsafe fn on_mousemove(mc: &mut MissionControl, hwnd: HWND, pt: POINT) {
    let old_hover_s = mc.hovered_space;
    let old_hover_w = mc.hovered_window;
    let old_hover_w_close = mc.hovered_window_close;
    let old_hover_w_pin = mc.hovered_window_pin;
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
            if (pt.x - mc.drag_offset.x).abs() > threshold_x && mc.space_cards.len() > 1 {
                mc.drag_space_active = true;
                mc.hovered_space = None;
                mc.hovered_close = None;
                mc.drag_paint_tick = 0;
                mc.drag_frame_ms = display::frame_interval_ms(hwnd);
            }
        }
        if mc.drag_space_active {
            let Some(orig_left) = mc.space_cards.get(drag_s_idx).map(|c| c.rect.left) else {
                return;
            };
            let count = mc.space_cards.len();
            let scale = mc.scale;
            let mut client_rect: RECT = std::mem::zeroed();
            GetClientRect(hwnd, &mut client_rect);
            let width = client_rect.right - client_rect.left;
            let bar = spaces_bar_metrics(count, mc.plus_visible, width, scale, mc.plus_label_w);

            let card_drag_left = orig_left + (pt.x - mc.drag_offset.x);
            let card_center_x = card_drag_left + bar.card_w / 2;

            let target =
                target_slot_for_center(card_center_x, bar.start_x, bar.card_w, bar.gap, count);
            // Nothing outside the spaces bar moves while a card is
            // dragged, so the strip is the whole dirty region —
            // and it is *always* the whole strip, never just the
            // band between two positions. Frames get dropped by
            // the throttle below, so the last painted position is
            // not the previous message's position, and a band
            // measured from the latter leaves the pixels the card
            // actually vacated unpainted: a trail of ghost cards.
            let moved =
                mc.drag_space_current_x != pt.x || mc.drag_space_target_slot != Some(target);
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
    hover_at(mc, pt);

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
            update_drag_ghost(mc, drag_idx, pt, hwnd);
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
    if old_hover_w_close != mc.hovered_window_close {
        for idx in [old_hover_w_close, mc.hovered_window_close]
            .into_iter()
            .flatten()
        {
            if let Some(card) = mc.window_cards.get(idx) {
                invalidate_hover_rect(hwnd, &card.card_rect);
            }
        }
    }
    if old_hover_w_pin != mc.hovered_window_pin {
        for idx in [old_hover_w_pin, mc.hovered_window_pin]
            .into_iter()
            .flatten()
        {
            if let Some(card) = mc.window_cards.get(idx) {
                invalidate_hover_rect(hwnd, &card.card_rect);
            }
        }
    }
}

pub(crate) unsafe extern "system" fn mc_wnd_proc(
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
            // Deliberately `double_buffer`, NOT `paint_surface`: this window
            // must let GDI's alpha=0 output reach the DWM acrylic backdrop
            // untouched, and `paint_surface` would promote it to opaque.
            double_buffer(hdc, &ps.rcPaint, |mem_dc| {
                render_mission_control(mem_dc, hwnd);
            });
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
                    super::hide_mission_control();
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
                    if let Some(h) = host() {
                        (h.reorder_space_neighbor)(mon_idx, delta);
                    }
                }
            } else if (0x31..=0x39).contains(&key)
                || (VK_NUMPAD1 as u32..=VK_NUMPAD9 as u32).contains(&key)
            {
                let space_idx = if key >= VK_NUMPAD1 as u32 {
                    (key - VK_NUMPAD1 as u32) as usize
                } else {
                    (key - 0x31) as usize
                };
                // Switch the monitor Mission Control is showing, not wherever
                // the cursor happens to be at keypress time — and stay open,
                // like the space-card click. Digits past this monitor's count
                // are no-ops.
                let mon_idx = MC_STATE.with(|s| s.borrow().active_mon_idx);
                if let Some(h) = host() {
                    (h.switch_space)(mon_idx, space_idx);
                }
            } else if key == 0x50
            /* VK_P */
            {
                // P pins/unpins the hovered window card. `wParam` here is a
                // virtual-key code, which has no lowercase form — the `0x70`
                // that used to sit alongside this as "lowercase p" is VK_F1,
                // so F1 toggled the pin.
                let hovered_win = MC_STATE.with(|s| {
                    let mc = s.borrow();
                    mc.hovered_window
                        .and_then(|idx| mc.window_cards.get(idx).map(|c| c.hwnd))
                });
                if let Some(target_hwnd) = hovered_win {
                    if let Some(h) = host() {
                        (h.toggle_window_sticky)(target_hwnd);
                    }
                }
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
                let bar = spaces_bar_metrics(
                    mc.space_cards.len(),
                    mc.plus_visible,
                    width,
                    mc.scale,
                    mc.plus_label_w,
                );
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
                // An armed close button never painted a pressed state, so
                // disarming it needs no repaint — just the state clear.
                mc.pressed_window_close = None;
                mc.pressed_window_pin = None;
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
            let pt = client_point(lparam);
            // The borrow lives exactly as long as the decision; every host
            // call below re-enters `MC_STATE` and would panic inside it.
            let intent = MC_STATE.with(|s| on_lbuttondown(&mut s.borrow_mut(), hwnd, pt));
            match intent {
                // The choke point persists the count, re-registers hotkeys
                // and refreshes the open overlay.
                DownIntent::AddSpace(mon) => {
                    if let Some(h) = host() {
                        (h.add_space)(mon);
                    }
                }
                DownIntent::RemoveSpace(mon, space) => {
                    if let Some(h) = host() {
                        (h.remove_space)(mon, space);
                    }
                }
                DownIntent::Hide => super::hide_mission_control(),
                DownIntent::None => {}
            }
            0
        }
        WM_MOUSEMOVE => {
            let pt = client_point(lparam);
            MC_STATE.with(|s| on_mousemove(&mut s.borrow_mut(), hwnd, pt));
            0
        }
        WM_LBUTTONUP => {
            let pt = client_point(lparam);
            let intent = MC_STATE.with(|s| on_lbuttonup(&mut s.borrow_mut(), hwnd, pt));

            // After the drag state is taken, never before: `ReleaseCapture`
            // dispatches `WM_CAPTURECHANGED` to this very wndproc, and that
            // handler clears both drag state machines.
            ReleaseCapture();

            match intent {
                UpIntent::ToggleSticky(target_hwnd) => {
                    if let Some(h) = host() {
                        (h.toggle_window_sticky)(target_hwnd);
                    }
                }
                UpIntent::CloseWindow(target_hwnd) => {
                    if let Some(h) = host() {
                        (h.close_window)(target_hwnd);
                    }
                }
                UpIntent::ReorderSpace { mon, from, to } => {
                    if let Some(h) = host() {
                        (h.reorder_space)(mon, from, to);
                    }
                }
                UpIntent::SwitchSpace { mon, space } => {
                    if let Some(h) = host() {
                        (h.switch_space)(mon, space);
                    }
                }
                UpIntent::MoveWindow {
                    hwnd: target_hwnd,
                    mon,
                    space,
                } => {
                    log_info!(
                        "Mission Control Drag&Drop: Moved window {:?} to Space {}",
                        target_hwnd,
                        space + 1
                    );
                    if let Some(h) = host() {
                        (h.move_window_to_space)(target_hwnd, mon, space);
                    }
                }
                UpIntent::NewSpaceWith {
                    hwnd: target_hwnd,
                    mon,
                } => {
                    if let Some(h) = host() {
                        (h.move_window_to_new_space)(target_hwnd, mon);
                    }
                }
                UpIntent::FocusWindow(focus_hwnd) => {
                    super::hide_mission_control();
                    SetForegroundWindow(focus_hwnd);
                }
                UpIntent::None => {}
            }
            0
        }
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

#[cfg(test)]
mod tests {
    use super::super::SpaceCard;
    use super::*;
    use std::ptr::null_mut;

    fn rect(left: i32, top: i32, right: i32, bottom: i32) -> RECT {
        RECT {
            left,
            top,
            right,
            bottom,
        }
    }

    fn center(r: &RECT) -> POINT {
        POINT {
            x: (r.left + r.right) / 2,
            y: (r.top + r.bottom) / 2,
        }
    }

    fn space_card(space_idx: usize, rect: RECT) -> SpaceCard {
        SpaceCard {
            space_idx,
            rect,
            window_count: 0,
            is_active: false,
            is_tiled: false,
        }
    }

    fn window_card(hwnd: usize, rect: RECT) -> WindowCard {
        WindowCard {
            hwnd: hwnd as HWND,
            h_thumb: 0,
            h_icon: null_mut(),
            card_rect: rect,
            thumb_rect: rect,
            title: String::new(),
            is_sticky: false,
        }
    }

    /// Two space cards, a "+" tile and one window card, at scale 1.
    fn overlay() -> MissionControl {
        let mut mc = MissionControl::new();
        mc.scale = 1.0;
        mc.space_cards = vec![
            space_card(0, rect(0, 0, 200, 100)),
            space_card(1, rect(220, 0, 420, 100)),
        ];
        mc.plus_rect = rect(440, 0, 540, 100);
        mc.plus_visible = true;
        mc.window_cards = vec![window_card(100, rect(0, 200, 300, 400))];
        mc
    }

    #[test]
    fn the_plus_tile_is_tested_first() {
        let mc = overlay();
        assert_eq!(press_target(&mc, center(&mc.plus_rect)), PressTarget::Plus);
        let mut hidden = overlay();
        hidden.plus_visible = false;
        assert_eq!(
            press_target(&hidden, center(&hidden.plus_rect)),
            PressTarget::Backdrop
        );
    }

    #[test]
    fn a_space_close_button_beats_the_card_only_with_more_than_one_space() {
        let mut mc = overlay();
        let close = close_button_rect(&mc.space_cards[1].rect, mc.scale);
        assert_eq!(
            press_target(&mc, center(&close)),
            PressTarget::SpaceClose(1)
        );

        mc.space_cards.truncate(1);
        let close = close_button_rect(&mc.space_cards[0].rect, mc.scale);
        assert_eq!(press_target(&mc, center(&close)), PressTarget::SpaceCard(0));
    }

    #[test]
    fn window_buttons_beat_the_card_body() {
        let mc = overlay();
        let card = mc.window_cards[0].card_rect;
        let pin = window_pin_button_rect(&card, mc.scale);
        let close = window_close_button_rect(&card, mc.scale);
        assert_eq!(press_target(&mc, center(&pin)), PressTarget::WindowPin(0));
        assert_eq!(
            press_target(&mc, center(&close)),
            PressTarget::WindowClose(0)
        );
        assert_eq!(
            press_target(&mc, POINT { x: 150, y: 300 }),
            PressTarget::WindowCard(0)
        );
    }

    #[test]
    fn cards_and_the_backdrop() {
        let mc = overlay();
        assert_eq!(
            press_target(&mc, POINT { x: 100, y: 50 }),
            PressTarget::SpaceCard(0)
        );
        assert_eq!(
            press_target(&mc, POINT { x: 300, y: 50 }),
            PressTarget::SpaceCard(1)
        );
        assert_eq!(
            press_target(&mc, POINT { x: 150, y: 150 }),
            PressTarget::Backdrop
        );
    }
}
