//! The daemon's implementation of Overview's [`OverviewHost`] vtable.
//!
//! Overview cannot name `AppState` — that would put the overlay's module
//! and the daemon in a call cycle. Instead the overlay declares what it needs
//! done and the daemon fills the table in at startup.
//!
//! Every function here is a *verbatim* wrapper around the `with_app_state`
//! block that used to sit inline in `overview.rs`'s wndproc. Same
//! borrow, taken at the same instant, released before returning: the drag state
//! machine spread across `WM_LBUTTONDOWN` / `WM_MOUSEMOVE` / `WM_LBUTTONUP` /
//! `WM_CAPTURECHANGED` depends on exactly which re-entrant events
//! `with_app_state` drops, and that must not shift.

use windows_sys::Win32::Foundation::HWND;
use windows_sys::Win32::UI::WindowsAndMessaging::{PostMessageW, SetTimer, WM_CLOSE};
use winspaces_common::log_info;

use crate::app::with_app_state;
use crate::handlers::session::{CLOSE_VERIFY_MS, TIMER_CLOSE_VERIFY};
use crate::spaces::{add_space_on, after_space_count_change, remove_space_on, reorder_space_on};
use winspaces_ui::overview::{neighbor_slot, refresh_overview, OverviewHost};

/// Installed once at startup; `'static` so the overlay can hold a plain
/// reference to it for the process lifetime.
pub(crate) static OVERVIEW_HOST: OverviewHost = OverviewHost {
    add_space,
    remove_space,
    reorder_space,
    reorder_space_neighbor,
    switch_space,
    move_window_to_space,
    move_window_to_new_space,
    close_window,
    toggle_window_sticky,
};

fn toggle_window_sticky(hwnd: HWND) {
    with_app_state(|state| {
        let now_sticky = state.space_mgr.toggle_sticky(hwnd);
        log_info!(
            "Overview: Toggled sticky for hwnd {:?} (now_sticky={})",
            hwnd,
            now_sticky
        );
        refresh_overview(&mut state.space_mgr);
    });
}

/// Ask a window to close, then re-check the overlay a moment later.
///
/// `WM_CLOSE` is a *request*: an app with unsaved work answers it with a
/// confirmation dialog and may never die, so untracking the window here would
/// clear its state prop and discard the space assignment of a window that is
/// still very much alive. Nothing about the daemon's state changes on the
/// click — `TIMER_CLOSE_VERIFY` simply rebuilds the grid once the app has had
/// its message cycle, and the card survives or disappears according to whether
/// the window did.
///
/// The verify pass is not belt-and-braces, it is the only prompt signal there
/// is: `HSHELL_WINDOWDESTROYED` arrives while the handle can still be alive,
/// and the liveness guard that makes it safe (our own cloaking makes the shell
/// fire it too) then drops the event without a retry.
fn close_window(hwnd: HWND) {
    with_app_state(|state| unsafe {
        PostMessageW(hwnd, WM_CLOSE, 0, 0);
        SetTimer(
            state.message_hwnd,
            TIMER_CLOSE_VERIFY,
            CLOSE_VERIFY_MS,
            None,
        );
    });
}

fn add_space(mon: usize) {
    with_app_state(|state| add_space_on(state, mon));
}

fn remove_space(mon: usize, space: usize) {
    with_app_state(|state| remove_space_on(state, mon, space));
}

fn reorder_space(mon: usize, from: usize, to: usize) {
    with_app_state(|state| reorder_space_on(state, mon, from, to));
}

/// Walk the monitor's *active* space one slot. The source index is the live
/// `current`, which the overlay's wndproc has no `SpaceManager` to read, so
/// the whole read-then-reorder happens under one borrow here — exactly as it
/// did inline.
fn reorder_space_neighbor(mon: usize, delta: i32) {
    with_app_state(|state| {
        let Some(monitor) = state.space_mgr.monitors.get(mon) else {
            return;
        };
        let (current, count) = (monitor.current, monitor.spaces.len());
        if let Some(target) = neighbor_slot(current, delta, count) {
            reorder_space_on(state, mon, current, target);
        }
    });
}

/// Switch a monitor to one of its spaces and re-sync the open overlay in place.
///
/// The bounds tests and the `current != space` test come from the digit-key path
/// and are load-bearing there: `switch_space` bounds-checks itself, but it
/// does *not* short-circuit a switch to the space already showing — it would
/// re-arm the foreground-suppression window on every monitor. Overview's
/// click-to-switch path already screens the same case against its own mirror of
/// `current` (`active_space_idx`), so the test is redundant, not new, there.
fn switch_space(mon: usize, space: usize) {
    with_app_state(|state| {
        if mon < state.space_mgr.monitors.len()
            && space < state.space_mgr.monitors[mon].spaces.len()
            && state.space_mgr.monitors[mon].current != space
        {
            state.space_mgr.switch_space(mon, space, None);
            refresh_overview(&mut state.space_mgr);
        }
    });
}

fn move_window_to_space(hwnd: HWND, mon: usize, space: usize) {
    with_app_state(|state| {
        state.space_mgr.track_window(hwnd, mon, space);
        refresh_overview(&mut state.space_mgr);
    });
}

/// Add a space to `mon` and land `hwnd` on it.
///
/// Deliberately NOT routed through `add_space_on`: that choke point never
/// tracks a window, and the dragged window has to be placed *between*
/// `add_space` and `after_space_count_change` so the persist/hotkey/tray/refresh
/// tail sees the window already on its new space. Converging the two needs a
/// variant choke point and is a behaviour change, not a refactor.
fn move_window_to_new_space(hwnd: HWND, mon: usize) {
    with_app_state(|state| {
        let old_max = state.space_mgr.max_space_count();
        if state.space_mgr.add_space(mon) {
            let new_last = state.space_mgr.monitors[mon].spaces.len() - 1;
            log_info!(
                "Overview Drag&Drop: window {:?} to new Space {}",
                hwnd,
                new_last + 1
            );
            state.space_mgr.track_window(hwnd, mon, new_last);
            after_space_count_change(state, old_max);
        } else {
            // At the cap (defensive; the tile is hidden then): rebuilding
            // restores the ghost to its grid slot.
            refresh_overview(&mut state.space_mgr);
        }
    });
}
