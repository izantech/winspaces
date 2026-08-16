//! Space-count and hotkey actions. Tray, Mission Control and the global
//! hotkeys all land on the choke points here so persistence, hotkey
//! registration, the tray badge and an open overlay never drift apart.

use windows_sys::Win32::UI::WindowsAndMessaging::{GetForegroundWindow, PostQuitMessage};
use winspaces_common::{log_debug, log_error, log_info, log_warn};
use winspaces_core::hotkeys::{
    HotkeyManager, HOTKEY_ID_EXIT, HOTKEY_ID_MISSION_CONTROL, HOTKEY_ID_MOVE_BASE,
    HOTKEY_ID_MOVE_NEXT, HOTKEY_ID_MOVE_PREV, HOTKEY_ID_NEXT, HOTKEY_ID_PREV,
    HOTKEY_ID_SPECIAL_BASE, HOTKEY_ID_SWITCH_BASE, HOTKEY_ID_TILING_FOCUS_DOWN,
    HOTKEY_ID_TILING_FOCUS_LEFT, HOTKEY_ID_TILING_FOCUS_RIGHT, HOTKEY_ID_TILING_FOCUS_UP,
    HOTKEY_ID_TILING_RATIO_GROW, HOTKEY_ID_TILING_RATIO_SHRINK, HOTKEY_ID_TILING_SWAP_DOWN,
    HOTKEY_ID_TILING_SWAP_LEFT, HOTKEY_ID_TILING_SWAP_RIGHT, HOTKEY_ID_TILING_SWAP_UP,
    HOTKEY_ID_TILING_TOGGLE, HOTKEY_ID_TILING_TOGGLE_FLOAT, HOTKEY_ID_TOGGLE,
    HOTKEY_ID_TOGGLE_STICKY,
};
use winspaces_core::layout_store;
use winspaces_core::tiling::Direction;
use winspaces_ui::mission_control;

use crate::app::{launch_settings, update_state_tray_icon, with_app_state, AppState};

pub(crate) fn handle_hotkey(id: i32) {
    log_debug!("Received WM_HOTKEY message for ID {}", id);
    if id == HOTKEY_ID_EXIT {
        log_info!("Hotkey exit requested");
        unsafe {
            PostQuitMessage(0);
        }
        return;
    }
    with_app_state(|state| {
        let mut space_changed = false;
        if (HOTKEY_ID_SWITCH_BASE..HOTKEY_ID_MOVE_BASE).contains(&id) {
            let space = (id - HOTKEY_ID_SWITCH_BASE) as usize;
            state.space_mgr.go_to_space(space);
            update_state_tray_icon(state);
            space_changed = true;
        } else if (HOTKEY_ID_MOVE_BASE..HOTKEY_ID_SPECIAL_BASE).contains(&id) {
            let space = (id - HOTKEY_ID_MOVE_BASE) as usize;
            state.space_mgr.move_to_space(space);
            update_state_tray_icon(state);
            space_changed = true;
        } else if id == HOTKEY_ID_PREV {
            state.space_mgr.step_space(-1);
            update_state_tray_icon(state);
            space_changed = true;
        } else if id == HOTKEY_ID_NEXT {
            state.space_mgr.step_space(1);
            update_state_tray_icon(state);
            space_changed = true;
        } else if id == HOTKEY_ID_MOVE_PREV {
            state.space_mgr.step_move_window(-1);
            update_state_tray_icon(state);
            space_changed = true;
        } else if id == HOTKEY_ID_MOVE_NEXT {
            state.space_mgr.step_move_window(1);
            update_state_tray_icon(state);
            space_changed = true;
        } else if id == HOTKEY_ID_TOGGLE {
            toggle_hotkeys(state);
        } else if id == HOTKEY_ID_MISSION_CONTROL {
            mission_control::toggle_mission_control(&mut state.space_mgr);
        } else if id == HOTKEY_ID_TOGGLE_STICKY {
            let fg = unsafe { GetForegroundWindow() };
            if !fg.is_null() && winspaces_core::spaces::is_valid_window(fg) {
                // Reports what the pin *became*, not what was asked for:
                // `set_sticky` refuses a window it does not track, and logs
                // its own reason when it does.
                let now_sticky = state.space_mgr.toggle_sticky(fg);
                log_info!(
                    "Hotkey toggle_sticky: hwnd {:?} (now_sticky={})",
                    fg,
                    now_sticky
                );
                if mission_control::is_mission_control_active() {
                    mission_control::refresh_mission_control(&mut state.space_mgr);
                }
            }
        } else if id == HOTKEY_ID_TILING_TOGGLE {
            toggle_tiling(state);
        } else if id == HOTKEY_ID_TILING_FOCUS_LEFT {
            state.space_mgr.tiling_focus(Direction::Left);
        } else if id == HOTKEY_ID_TILING_FOCUS_RIGHT {
            state.space_mgr.tiling_focus(Direction::Right);
        } else if id == HOTKEY_ID_TILING_FOCUS_UP {
            state.space_mgr.tiling_focus(Direction::Up);
        } else if id == HOTKEY_ID_TILING_FOCUS_DOWN {
            state.space_mgr.tiling_focus(Direction::Down);
        } else if id == HOTKEY_ID_TILING_SWAP_LEFT {
            state.space_mgr.tiling_swap(Direction::Left);
        } else if id == HOTKEY_ID_TILING_SWAP_RIGHT {
            state.space_mgr.tiling_swap(Direction::Right);
        } else if id == HOTKEY_ID_TILING_SWAP_UP {
            state.space_mgr.tiling_swap(Direction::Up);
        } else if id == HOTKEY_ID_TILING_SWAP_DOWN {
            state.space_mgr.tiling_swap(Direction::Down);
        } else if id == HOTKEY_ID_TILING_RATIO_SHRINK {
            let step = (state.config.tiling.ratio_step_pct as f32) / 100.0;
            state.space_mgr.tiling_adjust_ratio(-step);
        } else if id == HOTKEY_ID_TILING_RATIO_GROW {
            let step = (state.config.tiling.ratio_step_pct as f32) / 100.0;
            state.space_mgr.tiling_adjust_ratio(step);
        } else if id == HOTKEY_ID_TILING_TOGGLE_FLOAT {
            let fg = unsafe { GetForegroundWindow() };
            state.space_mgr.tiling_toggle_float(fg);
        }

        // Global switch/move hotkeys pressed with the overlay open should

        // update it in place, never dismiss it.
        if space_changed && mission_control::is_mission_control_active() {
            mission_control::refresh_mission_control(&mut state.space_mgr);
        }
    });
}

pub(crate) fn toggle_tiling(state: &mut AppState) {
    let next = !state.space_mgr.tiling_enabled;
    state.space_mgr.set_tiling_enabled(next);
    state.config.tiling.enabled = next;
    let path = winspaces_common::Config::get_config_path();
    let _ = state.config.save_to_file(&path);
    log_info!("Tiling toggled: enabled={}", next);
}

pub(crate) fn toggle_hotkeys(state: &mut AppState) {
    state.space_mgr.handle_hotkeys = !state.space_mgr.handle_hotkeys;
    if state.space_mgr.handle_hotkeys {
        if !HotkeyManager::register_all(&state.config, state.space_mgr.max_space_count()) {
            log_warn!("Hotkey re-registration failed upon toggle; opening settings window.");
            launch_settings();
            state.space_mgr.handle_hotkeys = false;
        }
    } else {
        HotkeyManager::unregister_all();
    }
}

/// Single choke points for changing a monitor's space count: tray and Mission
/// Control both land here, so persistence, hotkey registration, the tray badge
/// and an open overlay can never drift apart.
pub(crate) fn add_space_on(state: &mut AppState, mon_idx: usize) {
    let old_max = state.space_mgr.max_space_count();
    if state.space_mgr.add_space(mon_idx) {
        after_space_count_change(state, old_max);
    }
}

pub(crate) fn remove_space_on(state: &mut AppState, mon_idx: usize, space_idx: usize) {
    let old_max = state.space_mgr.max_space_count();
    if state.space_mgr.remove_space(mon_idx, space_idx) {
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
    if state.space_mgr.reorder_space(mon_idx, from_idx, to_idx) {
        update_state_tray_icon(state);
        if mission_control::is_mission_control_active() {
            mission_control::refresh_mission_control(&mut state.space_mgr);
        }
    }
}

pub(crate) fn after_space_count_change(state: &mut AppState, old_max: usize) {
    persist_space_counts(state);
    let new_max = state.space_mgr.max_space_count();
    if new_max != old_max && state.space_mgr.handle_hotkeys {
        HotkeyManager::unregister_all();
        if !HotkeyManager::register_all(&state.config, new_max) {
            log_warn!("Hotkey re-registration failed after space count change.");
        }
    }
    update_state_tray_icon(state);
    if mission_control::is_mission_control_active() {
        mission_control::refresh_mission_control(&mut state.space_mgr);
    }
}

/// Write the live per-monitor space counts straight into the stored topology
/// entry. The shadow path cannot be relied on for this: `shadow_tick` refuses
/// empty captures, so a count change with no windows open would never reach
/// disk. Cheap and user-initiated, so no debounce.
pub(crate) fn persist_space_counts(state: &mut AppState) {
    let signature = state.space_mgr.topology_signature();
    let live = layout_store::live_monitors(&state.space_mgr);

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
