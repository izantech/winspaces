//! The ghost preview during a window drag: a timer polls while the modal move
//! loop runs, so the Shift-held tile preview follows the key and the strip
//! lights up as a stage window's drop target, and the drop commits what the
//! preview showed.

use windows_sys::Win32::Foundation::{HWND, POINT, RECT};
use windows_sys::Win32::UI::WindowsAndMessaging::{GetCursorPos, GetWindowRect, SetTimer};

use std::cell::Cell;

use crate::app::with_app_state;
use crate::handlers::session::{DRAG_PREVIEW_INTERVAL_MS, TIMER_DRAG_PREVIEW};

/// A window's position and size.
type Frame = ((i32, i32), (i32, i32));

thread_local! {
    /// Whether the most recent drag-preview tick sampled Shift as held. Not
    /// sticky: it mirrors what the overlay showed on that tick, so the drop
    /// outcome (`this || Shift at drop`) can never disagree with the preview
    /// the user was looking at. Reset when the poll starts and stops.
    pub(super) static DRAG_PREVIEW_SHIFT: Cell<bool> = const { Cell::new(false) };
    /// The window being moved or sized and its frame when it started.
    static MOVESIZE_FROM: Cell<Option<(HWND, Frame)>> = const { Cell::new(None) };
}

fn frame_of(hwnd: HWND) -> Frame {
    let mut r = RECT {
        left: 0,
        top: 0,
        right: 0,
        bottom: 0,
    };
    unsafe { GetWindowRect(hwnd, &mut r) };
    ((r.left, r.top), (r.right - r.left, r.bottom - r.top))
}

/// A new position at the same size: a move, not a resize or a click.
fn moved(hwnd: HWND, (from_pos, from_size): Frame) -> bool {
    let (pos, size) = frame_of(hwnd);
    size == from_size && pos != from_pos
}

pub(super) fn begin_movesize(hwnd: HWND) {
    MOVESIZE_FROM.with(|f| f.set(Some((hwnd, frame_of(hwnd)))));
}

/// Whether the move/size that just ended on `hwnd` was a move.
pub(super) fn was_move(hwnd: HWND) -> bool {
    MOVESIZE_FROM
        .with(|f| f.take())
        .is_some_and(|(h, from)| h == hwnd && moved(h, from))
}

pub(super) fn shift_down() -> bool {
    unsafe {
        windows_sys::Win32::UI::Input::KeyboardAndMouse::GetAsyncKeyState(
            windows_sys::Win32::UI::Input::KeyboardAndMouse::VK_SHIFT as i32,
        ) as u16
            & 0x8000
            != 0
    }
}

/// Start the drag-preview poll. `SetTimer` on a live id just replaces the
/// pending wait, so a second START before an END costs nothing.
pub(super) fn start_drag_preview(hwnd: HWND) {
    DRAG_PREVIEW_SHIFT.with(|s| s.set(false));
    unsafe {
        SetTimer(hwnd, TIMER_DRAG_PREVIEW, DRAG_PREVIEW_INTERVAL_MS, None);
    }
}

/// Stop the drag-preview poll and drop the overlay. Safe to call when neither
/// is live — every MOVESIZEEND lands here whether or not a preview ran.
pub(super) fn stop_drag_preview(hwnd: HWND) {
    unsafe {
        windows_sys::Win32::UI::WindowsAndMessaging::KillTimer(hwnd, TIMER_DRAG_PREVIEW);
    }
    winspaces_ui::tiling_preview::hide_preview();
    DRAG_PREVIEW_SHIFT.with(|s| s.set(false));
}

/// One tick of the drag ghost preview (the Shift-drag tile toggle, or the
/// strip as a stage window's drop target), from the daemon's `WM_TIMER`
/// dispatch. Runs on the UI thread; the drag it serves may have already been
/// resolved elsewhere (tiling toggled off mid-drag), which is the tick's cue to
/// shut itself down. Over the strip the drop leaves the stage even with Shift
/// held, so the strip wins there.
pub(crate) fn on_drag_preview_tick(hwnd: HWND) {
    with_app_state(|state| {
        let drag = state.space_mgr.tiling_drag.as_ref();
        let from = MOVESIZE_FROM.with(|f| f.get());
        if drag.is_none() && !from.is_some_and(|(w, _)| state.space_mgr.shelf_can_leave(w)) {
            stop_drag_preview(hwnd);
            return;
        }

        let mut at = POINT { x: 0, y: 0 };
        unsafe { GetCursorPos(&mut at) };
        let zone = from
            .filter(|&(w, start)| moved(w, start))
            .and_then(|(w, _)| state.space_mgr.shelf_leave_zone(w, at));
        let over_strip = zone.as_ref().is_some_and(|z| z.contains(at));

        let shift_held = shift_down();
        DRAG_PREVIEW_SHIFT.with(|s| s.set(shift_held));
        let toggle = drag.filter(|_| shift_held && !over_strip).and_then(|d| {
            state
                .space_mgr
                .tiling_preview_toggle_rects(d.mon_idx, d.space_idx)
        });
        match (toggle, zone) {
            (Some((work, rects)), _) => winspaces_ui::tiling_preview::show_preview(&work, &rects),
            (None, Some(z)) => winspaces_ui::tiling_preview::show_preview(&RECT::from(&z), &[z]),
            (None, None) => winspaces_ui::tiling_preview::hide_preview(),
        }
    });
}
