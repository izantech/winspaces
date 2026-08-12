//! The daemon's implementation of Mission Control's [`McHost`] vtable.
//!
//! Mission Control cannot name `AppState` — that would put the overlay's module
//! and the daemon in a call cycle. Instead the overlay declares what it needs
//! done and the daemon fills the table in at startup.
//!
//! Every function here is a *verbatim* wrapper around the `with_app_state`
//! block that used to sit inline in `mission_control.rs`'s wndproc. Same
//! borrow, taken at the same instant, released before returning: the drag state
//! machine spread across `WM_LBUTTONDOWN` / `WM_MOUSEMOVE` / `WM_LBUTTONUP` /
//! `WM_CAPTURECHANGED` depends on exactly which re-entrant events
//! `with_app_state` drops, and that must not shift.

use windows_sys::Win32::Foundation::HWND;
use winspaces_common::log_info;

use crate::app::with_app_state;
use crate::spaces::{add_space_on, after_space_count_change, remove_space_on, reorder_space_on};
use winspaces_ui::mission_control::{neighbor_slot, refresh_mission_control, McHost};

/// Installed once at startup; `'static` so the overlay can hold a plain
/// reference to it for the process lifetime.
pub static MC_HOST: McHost = McHost {
    add_space,
    remove_space,
    reorder_space,
    reorder_space_neighbor,
    switch_space,
    move_window_to_space,
    move_window_to_new_space,
};

fn add_space(mon: usize) {
    with_app_state(|state| add_space_on(state, mon));
}

fn remove_space(mon: usize, desk: usize) {
    with_app_state(|state| remove_space_on(state, mon, desk));
}

fn reorder_space(mon: usize, from: usize, to: usize) {
    with_app_state(|state| reorder_space_on(state, mon, from, to));
}

/// Walk the monitor's *active* space one slot. The source index is the live
/// `current`, which the overlay's wndproc has no `DesktopManager` to read, so
/// the whole read-then-reorder happens under one borrow here — exactly as it
/// did inline.
fn reorder_space_neighbor(mon: usize, delta: i32) {
    with_app_state(|state| {
        let Some(monitor) = state.desktop_mgr.monitors.get(mon) else {
            return;
        };
        let (current, count) = (monitor.current, monitor.desktops.len());
        if let Some(target) = neighbor_slot(current, delta, count) {
            reorder_space_on(state, mon, current, target);
        }
    });
}

/// Switch a monitor to one of its spaces and re-sync the open overlay in place.
///
/// The bounds tests and the `current != desk` test come from the digit-key path
/// and are load-bearing there: `switch_desktop` bounds-checks itself, but it
/// does *not* short-circuit a switch to the space already showing — it would
/// re-arm the foreground-suppression window on every monitor. Mission Control's
/// click-to-switch path already screens the same case against its own mirror of
/// `current` (`active_desk_idx`), so the test is redundant, not new, there.
fn switch_space(mon: usize, desk: usize) {
    with_app_state(|state| {
        if mon < state.desktop_mgr.monitors.len()
            && desk < state.desktop_mgr.monitors[mon].desktops.len()
            && state.desktop_mgr.monitors[mon].current != desk
        {
            state.desktop_mgr.switch_desktop(mon, desk, None);
            refresh_mission_control(&mut state.desktop_mgr);
        }
    });
}

fn move_window_to_space(hwnd: HWND, mon: usize, desk: usize) {
    with_app_state(|state| {
        state.desktop_mgr.track_window(hwnd, mon, desk);
        refresh_mission_control(&mut state.desktop_mgr);
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
        let old_max = state.desktop_mgr.max_space_count();
        if state.desktop_mgr.add_space(mon) {
            let new_last = state.desktop_mgr.monitors[mon].desktops.len() - 1;
            log_info!(
                "Mission Control Drag&Drop: window {:?} to new Space {}",
                hwnd,
                new_last + 1
            );
            state.desktop_mgr.track_window(hwnd, mon, new_last);
            after_space_count_change(state, old_max);
        } else {
            // At the cap (defensive; the tile is hidden then): rebuilding
            // restores the ghost to its grid slot.
            refresh_mission_control(&mut state.desktop_mgr);
        }
    });
}
