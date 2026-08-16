//! Session, timer and lifecycle messages: display/RDP topology changes, WTS
//! session transitions, the reconcile/snapshot/persist timers, and process
//! teardown.

use windows_sys::Win32::Foundation::{HWND, WPARAM};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    KillTimer, PostQuitMessage, SetTimer, WA_INACTIVE,
};
use winspaces_common::log_info;
use winspaces_ui::menu;

use crate::app::{update_state_tray_icon, with_app_state};
use crate::shadow::{persist_shadow, reconcile_topology, shadow_tick};

pub(crate) const TIMER_RECONCILE: usize = 1;
pub(crate) const TIMER_SNAPSHOT: usize = 2;
pub(crate) const TIMER_PERSIST: usize = 3;
pub(crate) const TIMER_RESTORE_VERIFY: usize = 4;
pub(crate) const TIMER_CLOSE_VERIFY: usize = 5;

/// A topology change arrives as a burst of `WM_DISPLAYCHANGE` messages while
/// the OS is still reflowing windows. Wait for the dust to settle, then
/// reconcile once.
const RECONCILE_DEBOUNCE_MS: u32 = 1200;
/// How often the live layout is re-shadowed. `WM_DISPLAYCHANGE` fires *after*
/// windows have already been reflowed, so the layout worth restoring has to be
/// recorded continuously, before anything goes wrong.
pub(crate) const SNAPSHOT_INTERVAL_MS: u32 = 5000;
/// Layout changes constantly; the disk does not need to hear about every drag.
/// Kept short because a daemon restart or crash inside the window loses the
/// arrangement — a 30 s debounce once replayed a four-minute-old snapshot on
/// boot, reverting spaces the user had since rearranged.
pub(crate) const PERSIST_DEBOUNCE_MS: u32 = 5_000;
/// One-shot sweep after a topology restore, timed to land after Windows'
/// "remember window locations" reconnect sweep (~10 s after a monitor
/// returns) so any window it moved off its restored monitor is pushed back
/// even if no scan happens to run. Must stay inside the enforcement window
/// (`layout_store::RESTORE_ENFORCE_MS`).
pub(crate) const RESTORE_VERIFY_MS: u32 = 12_000;
/// How long after a Mission Control close request the overlay re-checks the
/// grid. An app that honours `WM_CLOSE` destroys its window within a message
/// cycle, so this only has to outlast the round trip — but the overlay cannot
/// simply drop the card on the click, because an app with unsaved work answers
/// `WM_CLOSE` with a dialog and stays alive. Re-filtering after the fact is
/// what makes the card's disappearance *mean* the window really went away.
///
/// The shell's own `HSHELL_WINDOWDESTROYED` is not a substitute: it fires
/// while the handle can still be alive, and its liveness guard (load-bearing
/// for cloak-induced notifications) then drops the event with no retry.
pub(crate) const CLOSE_VERIFY_MS: u32 = 150;

// Session-change reasons for WM_WTSSESSION_CHANGE (not exposed by windows-sys).
const WTS_CONSOLE_CONNECT: usize = 0x1;
const WTS_CONSOLE_DISCONNECT: usize = 0x2;
const WTS_REMOTE_CONNECT: usize = 0x3;
const WTS_REMOTE_DISCONNECT: usize = 0x4;

pub(crate) fn on_activate(wparam: WPARAM) {
    // The custom tray menu never activates; this hidden window is made
    // foreground instead when the menu opens. Losing that foreground status
    // (Alt-Tab, click into another app) is one of the menu's light-dismiss
    // signals.
    if (wparam & 0xffff) as u32 == WA_INACTIVE {
        menu::handle_owner_deactivate();
    }
}

pub(crate) fn on_display_change(hwnd: HWND) {
    // Debounced: a dock, undock or RDP transition fires several of these
    // while the OS is still relocating windows. Acting on the first one
    // records a half-finished topology.
    log_info!("Display topology changed; scheduling reconcile");
    with_app_state(|state| {
        state.space_mgr.reconcile_pending = true;
    });
    unsafe {
        SetTimer(hwnd, TIMER_RECONCILE, RECONCILE_DEBOUNCE_MS, None);
    }
}

pub(crate) fn on_wtssession_change(hwnd: HWND, wparam: WPARAM) {
    let reason = match wparam {
        WTS_CONSOLE_CONNECT => "console connect",
        WTS_CONSOLE_DISCONNECT => "console disconnect",
        WTS_REMOTE_CONNECT => "remote connect",
        WTS_REMOTE_DISCONNECT => "remote disconnect",
        _ => "other",
    };
    log_info!("Session change: {} ({})", reason, wparam);
    if matches!(
        wparam,
        WTS_CONSOLE_CONNECT | WTS_CONSOLE_DISCONNECT | WTS_REMOTE_CONNECT | WTS_REMOTE_DISCONNECT
    ) {
        // The display swap that accompanies an RDP transition can land either
        // side of this message, so join the same debounce rather than
        // reconciling here.
        with_app_state(|state| {
            state.space_mgr.reconcile_pending = true;
        });
        unsafe {
            SetTimer(hwnd, TIMER_RECONCILE, RECONCILE_DEBOUNCE_MS, None);
        }
    }
}

pub(crate) fn on_timer(hwnd: HWND, wparam: WPARAM) {
    match wparam {
        TIMER_RECONCILE => {
            unsafe {
                KillTimer(hwnd, TIMER_RECONCILE);
            }
            with_app_state(|state| {
                reconcile_topology(state);
                update_state_tray_icon(state);
            });
        }
        TIMER_SNAPSHOT => with_app_state(shadow_tick),
        TIMER_PERSIST => with_app_state(persist_shadow),
        TIMER_CLOSE_VERIFY => {
            unsafe {
                KillTimer(hwnd, TIMER_CLOSE_VERIFY);
            }
            with_app_state(|state| {
                if winspaces_ui::mission_control::is_mission_control_active() {
                    // Rebuilding re-runs the eligibility filter, so a window
                    // that actually died loses its card here. One that put up a
                    // "save changes?" dialog is still live and keeps its card —
                    // which is the honest answer, not a stale one.
                    winspaces_ui::mission_control::refresh_mission_control(&mut state.space_mgr);
                }
            });
        }
        TIMER_RESTORE_VERIFY => {
            unsafe {
                KillTimer(hwnd, TIMER_RESTORE_VERIFY);
            }
            with_app_state(|state| {
                let pushed = state.space_mgr.enforce_restore_pass();
                if pushed > 0 {
                    log_info!(
                        "restore-verify: pushed {} drifted window(s) back after topology restore",
                        pushed
                    );
                }
            });
        }
        _ => {}
    }
}

pub(crate) fn on_end_session(wparam: WPARAM) {
    // Logoff/shutdown previously skipped cleanup entirely, leaving windows
    // cloaked for the next session to reclaim. Save the layout and un-hide
    // everything while there is still time.
    if wparam != 0 {
        log_info!("Session ending; persisting layout and restoring windows");
        with_app_state(|state| {
            persist_shadow(state);
            state.space_mgr.windows_show_all();
        });
    }
}

pub(crate) fn on_destroy() {
    log_info!("Window WM_DESTROY received");
    with_app_state(|state| {
        state.tray_icon.remove();
    });
    unsafe {
        PostQuitMessage(0);
    }
}
