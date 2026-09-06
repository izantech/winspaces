//! The Shift-held preview during a tiled window drag: a timer polls the
//! modifier while the modal move loop runs, so the ghost preview follows the
//! key and the drop commits what the preview showed.

use windows_sys::Win32::Foundation::HWND;

use std::cell::Cell;

use crate::app::with_app_state;
use crate::handlers::session::TIMER_DRAG_PREVIEW;

thread_local! {
    /// Whether the most recent drag-preview tick sampled Shift as held. Not
    /// sticky: it mirrors what the overlay showed on that tick, so the drop
    /// outcome (`this || Shift at drop`) can never disagree with the preview
    /// the user was looking at. Reset when the poll starts and stops.
    pub(super) static DRAG_PREVIEW_SHIFT: Cell<bool> = const { Cell::new(false) };
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

/// Stop the drag-preview poll and drop the overlay. Safe to call when neither
/// is live — every MOVESIZEEND lands here whether or not a preview ran.
pub(super) fn stop_drag_preview(hwnd: HWND) {
    unsafe {
        windows_sys::Win32::UI::WindowsAndMessaging::KillTimer(hwnd, TIMER_DRAG_PREVIEW);
    }
    winspaces_ui::tiling_preview::hide_preview();
    DRAG_PREVIEW_SHIFT.with(|s| s.set(false));
}

/// One tick of the Shift-drag ghost preview,
/// from the daemon's `WM_TIMER` dispatch. Runs on the UI thread; the drag it
/// serves may have already been resolved elsewhere (tiling toggled off mid-drag),
/// which is the tick's cue to shut itself down.
pub(crate) fn on_drag_preview_tick(hwnd: HWND) {
    with_app_state(|state| {
        let Some(drag) = state.space_mgr.tiling_drag.clone() else {
            stop_drag_preview(hwnd);
            return;
        };

        let shift_held = shift_down();
        DRAG_PREVIEW_SHIFT.with(|s| s.set(shift_held));
        if !shift_held {
            winspaces_ui::tiling_preview::hide_preview();
            return;
        }
        match state
            .space_mgr
            .tiling_preview_toggle_rects(drag.mon_idx, drag.space_idx)
        {
            Some((work, rects)) => winspaces_ui::tiling_preview::show_preview(&work, &rects),
            None => winspaces_ui::tiling_preview::hide_preview(),
        }
    });
}
