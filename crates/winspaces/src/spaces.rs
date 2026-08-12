//! Space-count and hotkey actions. Tray, Mission Control and the global
//! hotkeys all land on the choke points here so persistence, hotkey
//! registration, the tray badge and an open overlay never drift apart.

use windows_sys::Win32::UI::WindowsAndMessaging::PostQuitMessage;
use winspaces_common::{log_error, log_info, log_warn};
use winspaces_core::hotkeys::{
    HotkeyManager, HOTKEY_ID_EXIT, HOTKEY_ID_MISSION_CONTROL, HOTKEY_ID_MOVE_BASE,
    HOTKEY_ID_MOVE_NEXT, HOTKEY_ID_MOVE_PREV, HOTKEY_ID_NEXT, HOTKEY_ID_PREV,
    HOTKEY_ID_SPECIAL_BASE, HOTKEY_ID_SWITCH_BASE, HOTKEY_ID_TOGGLE,
};
use winspaces_core::layout_store;
use winspaces_ui::mission_control;

use crate::app::{launch_settings, update_state_tray_icon, with_app_state, AppState};

pub(crate) fn handle_hotkey(id: i32) {
    log_info!("Received WM_HOTKEY message for ID {}", id);
    if id == HOTKEY_ID_EXIT {
        log_info!("Hotkey exit requested");
        unsafe {
            PostQuitMessage(0);
        }
        return;
    }
    with_app_state(|state| {
        let mut desktop_changed = false;
        if (HOTKEY_ID_SWITCH_BASE..HOTKEY_ID_MOVE_BASE).contains(&id) {
            let desk = (id - HOTKEY_ID_SWITCH_BASE) as usize;
            state.desktop_mgr.go_to_desk(desk);
            update_state_tray_icon(state);
            desktop_changed = true;
        } else if (HOTKEY_ID_MOVE_BASE..HOTKEY_ID_SPECIAL_BASE).contains(&id) {
            let desk = (id - HOTKEY_ID_MOVE_BASE) as usize;
            state.desktop_mgr.move_to_desk(desk);
            update_state_tray_icon(state);
            desktop_changed = true;
        } else if id == HOTKEY_ID_PREV {
            state.desktop_mgr.step_desktop(-1);
            update_state_tray_icon(state);
            desktop_changed = true;
        } else if id == HOTKEY_ID_NEXT {
            state.desktop_mgr.step_desktop(1);
            update_state_tray_icon(state);
            desktop_changed = true;
        } else if id == HOTKEY_ID_MOVE_PREV {
            state.desktop_mgr.step_move_window(-1);
            update_state_tray_icon(state);
            desktop_changed = true;
        } else if id == HOTKEY_ID_MOVE_NEXT {
            state.desktop_mgr.step_move_window(1);
            update_state_tray_icon(state);
            desktop_changed = true;
        } else if id == HOTKEY_ID_TOGGLE {
            toggle_hotkeys(state);
        } else if id == HOTKEY_ID_MISSION_CONTROL {
            mission_control::toggle_mission_control(&mut state.desktop_mgr);
        }

        // Global switch/move hotkeys pressed with the overlay open should
        // update it in place, never dismiss it.
        if desktop_changed && mission_control::is_mission_control_active() {
            mission_control::refresh_mission_control(&mut state.desktop_mgr);
        }
    });
}

pub(crate) fn toggle_hotkeys(state: &mut AppState) {
    state.desktop_mgr.handle_hotkeys = !state.desktop_mgr.handle_hotkeys;
    if state.desktop_mgr.handle_hotkeys {
        if !HotkeyManager::register_all(&state.config, state.desktop_mgr.max_space_count()) {
            log_warn!("Hotkey re-registration failed upon toggle; opening settings window.");
            launch_settings();
            state.desktop_mgr.handle_hotkeys = false;
        }
    } else {
        HotkeyManager::unregister_all();
    }
}

/// Single choke points for changing a monitor's space count: tray and Mission
/// Control both land here, so persistence, hotkey registration, the tray badge
/// and an open overlay can never drift apart.
pub(crate) fn add_space_on(state: &mut AppState, mon_idx: usize) {
    let old_max = state.desktop_mgr.max_space_count();
    if state.desktop_mgr.add_space(mon_idx) {
        after_space_count_change(state, old_max);
    }
}

pub(crate) fn remove_space_on(state: &mut AppState, mon_idx: usize, desk_idx: usize) {
    let old_max = state.desktop_mgr.max_space_count();
    if state.desktop_mgr.remove_space(mon_idx, desk_idx) {
        after_space_count_change(state, old_max);
    }
}

/// Choke point for moving a space within a monitor — Mission Control's card
/// drag and its `Ctrl+Shift+←/→` equivalent both land here. Unlike add/remove
/// the space count is unchanged, so there are no digit hotkeys to re-register
/// and no count to persist; the windows travel with the space, so the next
/// `shadow_tick` capture writes their new `space_index` values to disk.
pub(crate) fn reorder_space_on(
    state: &mut AppState,
    mon_idx: usize,
    from_idx: usize,
    to_idx: usize,
) {
    if state.desktop_mgr.reorder_space(mon_idx, from_idx, to_idx) {
        update_state_tray_icon(state);
        if mission_control::is_mission_control_active() {
            mission_control::refresh_mission_control(&mut state.desktop_mgr);
        }
    }
}

pub(crate) fn after_space_count_change(state: &mut AppState, old_max: usize) {
    persist_space_counts(state);
    let new_max = state.desktop_mgr.max_space_count();
    if new_max != old_max && state.desktop_mgr.handle_hotkeys {
        HotkeyManager::unregister_all();
        if !HotkeyManager::register_all(&state.config, new_max) {
            log_warn!("Hotkey re-registration failed after space count change.");
        }
    }
    update_state_tray_icon(state);
    if mission_control::is_mission_control_active() {
        mission_control::refresh_mission_control(&mut state.desktop_mgr);
    }
}

/// Write the live per-monitor space counts straight into the stored topology
/// entry. The shadow path cannot be relied on for this: `shadow_tick` refuses
/// empty captures, so a count change with no windows open would never reach
/// disk. Cheap and user-initiated, so no debounce.
pub(crate) fn persist_space_counts(state: &mut AppState) {
    let signature = state.desktop_mgr.topology_signature();
    let live = layout_store::live_monitors(&state.desktop_mgr);

    if let Some(entry) = state
        .layouts
        .topologies
        .iter_mut()
        .find(|t| t.signature == signature)
    {
        for mon in &mut entry.monitors {
            if let Some(live_mon) = live.iter().find(|l| l.stable_id == mon.stable_id) {
                mon.space_count = live_mon.space_count;
            }
        }
    } else {
        state.layouts.upsert(winspaces_common::TopologySnapshot {
            signature: signature.clone(),
            monitors: live.clone(),
            windows: Vec::new(),
            captured_unix: winspaces_common::unix_now(),
        });
    }

    if let Err(e) = state
        .layouts
        .save_to_file(&winspaces_common::LayoutStore::get_path())
    {
        log_error!(
            "Failed to save layouts.json after space count change: {}",
            e
        );
    }

    // Mirror the counts into the shadow so the next shadow_tick diff doesn't
    // immediately mark it dirty and rewrite the file for the same change.
    if let Some(shadow) = state.shadow.as_mut() {
        if shadow.signature == signature {
            for mon in &mut shadow.monitors {
                if let Some(live_mon) = live.iter().find(|l| l.stable_id == mon.stable_id) {
                    mon.space_count = live_mon.space_count;
                }
            }
        }
    }
}
