//! Live-layout shadowing: continuously capture the current arrangement so a
//! crash or topology change has something recent to restore, and reconcile a
//! settled topology change against the stored layout for its signature.

use windows_sys::Win32::UI::WindowsAndMessaging::{KillTimer, SetTimer};
use winspaces_common::{log_error, log_info, LayoutStore};
use winspaces_core::{layout_store, topology};

use crate::app::AppState;
use crate::handlers::session::{
    PERSIST_DEBOUNCE_MS, RESTORE_VERIFY_MS, TIMER_PERSIST, TIMER_RESTORE_VERIFY,
};

/// Settle a display-topology change: rebuild the monitor table, then replay the
/// stored layout if this topology is one we have seen before.
///
/// Runs once per burst, on the debounce timer rather than inline in
/// `WM_DISPLAYCHANGE`, because RDP connect/disconnect emits several of those
/// while the OS is still moving windows around.
pub(crate) fn reconcile_topology(state: &mut AppState) {
    state.space_mgr.handle_display_change();
    state.space_mgr.reconcile_pending = false;

    let signature = state.space_mgr.topology_signature();
    let remote = topology::is_remote_session();
    if signature == state.last_signature {
        log_info!("Topology unchanged after settle [{}]", signature);
        return;
    }
    log_info!(
        "Topology settled: [{}] -> [{}] (remote session: {})",
        state.last_signature,
        signature,
        remote
    );
    state.last_signature = signature.clone();
    // The shadow described the *previous* topology; drop it so the next tick
    // captures this one from scratch instead of diffing against stale data.
    state.shadow = None;

    if remote {
        // The physical monitors are detached and the RDP virtual display owns
        // the desktop. Whatever the layout looks like here is disposable — and
        // because it carries its own signature it can never overwrite the desk
        // layout on disk.
        log_info!("Remote session active; layout shadowing paused");
        return;
    }

    match state.layouts.find(&signature).cloned() {
        Some(snapshot) => {
            layout_store::restore_snapshot(&mut state.space_mgr, &snapshot);
            // Sweep once after Windows' own reconnect window-moving has had
            // its say, pushing back anything it moved off the restored layout.
            unsafe {
                SetTimer(
                    state.message_hwnd,
                    TIMER_RESTORE_VERIFY,
                    RESTORE_VERIFY_MS,
                    None,
                );
            }
        }
        None => {
            log_info!(
                "No stored layout for [{}]; leaving windows where the OS put them",
                signature
            );
        }
    }
}

/// Re-shadow the live layout. Cheap enough to run on a timer: one walk over
/// the tracked set, with the per-window identity answered from cache.
pub(crate) fn shadow_tick(state: &mut AppState) {
    // Never shadow mid-transition: a capture taken while the OS is still moving
    // windows would promote the scramble into the stored reference layout. The
    // enforcement window counts as mid-transition — the OS reconnect sweep may
    // still be moving restored windows, and a capture taken then would save
    // the drift the enforcement is about to undo.
    if state.space_mgr.reconcile_pending
        || state.space_mgr.is_settling()
        || state.space_mgr.is_enforcing_restore()
        || topology::is_remote_session()
    {
        return;
    }

    let snapshot = layout_store::capture_snapshot(&state.space_mgr);
    // An empty capture means the scan raced a teardown; never promote it over a
    // good layout.
    if snapshot.windows.is_empty() {
        return;
    }
    if state
        .shadow
        .as_ref()
        .is_some_and(|prev| layout_store::same_layout(prev, &snapshot))
    {
        return;
    }

    state.shadow = Some(snapshot);
    state.shadow_dirty = true;
    unsafe {
        SetTimer(state.message_hwnd, TIMER_PERSIST, PERSIST_DEBOUNCE_MS, None);
    }
}

pub(crate) fn persist_shadow(state: &mut AppState) {
    unsafe {
        KillTimer(state.message_hwnd, TIMER_PERSIST);
    }
    if !state.shadow_dirty {
        return;
    }
    let Some(snapshot) = state.shadow.clone() else {
        return;
    };
    let windows = snapshot.windows.len();
    let signature = snapshot.signature.clone();
    state.layouts.upsert(snapshot);
    match state.layouts.save_to_file(&LayoutStore::get_path()) {
        Ok(()) => {
            state.shadow_dirty = false;
            log_info!("Saved layout for [{}]: {} windows", signature, windows);
        }
        Err(e) => {
            log_error!("Failed to save layouts.json: {}", e);
        }
    }
}
