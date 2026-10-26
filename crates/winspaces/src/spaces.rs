//! Space-count and hotkey actions. Tray, Overview and the global
//! hotkeys all land on the choke points here so persistence, hotkey
//! registration, the tray badge and an open overlay never drift apart.

use std::cell::{Cell, RefCell};
use std::ptr::null_mut;
use windows_sys::Win32::Foundation::{HWND, POINT};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    GetForegroundWindow, KillTimer, PostQuitMessage,
};
use winspaces_common::i18n::t;
use winspaces_common::{log_debug, log_error, log_info, log_warn, Msg, ShelfSide};
use winspaces_ui::menu::{self, MenuEntry, MenuItemData};
use winspaces_win32::glyphs::{GLYPH_CLOSE, GLYPH_DOCK_LEFT, GLYPH_PIN, GLYPH_RESTORE};

use winspaces_core::hotkeys::{decode_hotkey, HotkeyAction, HotkeyManager};
use winspaces_core::layout_store;
use winspaces_core::spaces::fills_monitor;
use winspaces_ui::overview;
use winspaces_ui::shelf::{self, StripModel};

use crate::app::{launch_settings, update_state_tray_icon, with_app_state, AppState};

pub(crate) fn handle_hotkey(id: i32) {
    log_debug!("Received WM_HOTKEY message for ID {}", id);
    let Some(action) = decode_hotkey(id) else {
        log_warn!("WM_HOTKEY for an id this daemon never registered: {}", id);
        return;
    };
    if action == HotkeyAction::Exit {
        log_info!("Hotkey exit requested");
        unsafe {
            PostQuitMessage(0);
        }
        return;
    }
    with_app_state(|state| {
        match action {
            HotkeyAction::Exit => {}
            HotkeyAction::SwitchTo(space) => {
                state.space_mgr.go_to_space(space);
                update_state_tray_icon(state);
            }
            HotkeyAction::MoveTo(space) => {
                state.space_mgr.move_to_space(space);
                update_state_tray_icon(state);
            }
            HotkeyAction::StepSpace(delta) => {
                state.space_mgr.step_space(delta);
                update_state_tray_icon(state);
            }
            HotkeyAction::StepMove(delta) => {
                state.space_mgr.step_move_window(delta);
                update_state_tray_icon(state);
            }
            HotkeyAction::AuxToggle => {
                state.space_mgr.toggle_aux_space();
                update_state_tray_icon(state);
            }
            HotkeyAction::AuxMove => {
                state.space_mgr.move_to_aux_space();
                update_state_tray_icon(state);
            }
            HotkeyAction::ToggleHotkeys => toggle_hotkeys(state),
            HotkeyAction::Overview => {
                overview::toggle_overview(&mut state.space_mgr);
            }
            HotkeyAction::ToggleSticky => {
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
                    if overview::is_overview_active() {
                        overview::refresh_overview(&mut state.space_mgr);
                    }
                }
            }
            HotkeyAction::ShelfToggle => {
                let mon_idx = state.space_mgr.get_active_monitor_index();
                toggle_shelf(state, mon_idx);
            }
            HotkeyAction::TilingToggle => toggle_tiling(state),
            HotkeyAction::TilingFocus(direction) => state.space_mgr.tiling_focus(direction),
            HotkeyAction::TilingSwap(direction) => state.space_mgr.tiling_swap(direction),
            HotkeyAction::TilingRatio(sign) => {
                let step = (state.config.tiling.ratio_step_pct as f32) / 100.0;
                state.space_mgr.tiling_adjust_ratio(step * sign as f32);
            }
            HotkeyAction::TilingToggleFloat => {
                let fg = unsafe { GetForegroundWindow() };
                state.space_mgr.tiling_toggle_float(fg);
            }
            HotkeyAction::TilingToggleSplit => {
                if let Some(notice) = state.space_mgr.tiling_toggle_split() {
                    winspaces_ui::space_indicator::show_split_toast(&notice);
                }
            }
        }

        // Global switch/move hotkeys pressed with the overlay open should
        // update it in place, never dismiss it.
        if action.changes_space() && overview::is_overview_active() {
            overview::refresh_overview(&mut state.space_mgr);
        }
    });
}

pub(crate) fn toggle_tiling(state: &mut AppState) {
    let next = !state.space_mgr.tiling_enabled;
    state.space_mgr.set_tiling_enabled(next);
    state.config.tiling.enabled = next;
    crate::app::persist_config(&state.config);
    log_info!("Tiling toggled: enabled={}", next);
}

/// Shelf toggle for the hotkey and Overview: the display's current space, or
/// every space when the setting says so, in which case the state persists.
pub(crate) fn toggle_shelf(state: &mut AppState, mon_idx: usize) {
    let everywhere = state.config.shelf.all_spaces;
    let next = state.space_mgr.toggle_shelf(mon_idx, everywhere);
    if everywhere {
        state.config.shelf.enabled = next;
        crate::app::persist_config(&state.config);
    }
    persist_space_structure(state);
    refresh_shelf(state);
    if overview::is_overview_active() {
        overview::refresh_overview(&mut state.space_mgr);
    }
}

thread_local! {
    /// Each display's stage and whether it filled its monitor at the last
    /// refresh, so the location hook can tell when the strip must step aside.
    static STAGES: RefCell<Vec<(HWND, bool)>> = const { RefCell::new(Vec::new()) };
}

/// The one Shelf refresh: commit the roles, then make the screen and the
/// strips match. The strip steps aside while its stage fills the monitor.
pub(crate) fn refresh_shelf(state: &mut AppState) {
    let mgr = &mut state.space_mgr;
    mgr.shelf_apply();
    let mut stages = Vec::new();
    let models: Vec<Option<StripModel>> = (0..mgr.monitors.len())
        .map(|m| {
            let filled = mgr.shelf_stage_of(m).is_some_and(|h| {
                let filled = fills_monitor(h);
                stages.push((h, filled));
                filled
            });
            if filled {
                return None;
            }
            mgr.shelf_strip(m)
                .map(|(rect, cards)| StripModel { rect, cards })
        })
        .collect();
    shelf::sync(&models, overview::visible_hwnd());
    STAGES.with(|s| *s.borrow_mut() = stages);
    // Whatever this pass scheduled (its own retile marks) it has just done.
    unsafe {
        KillTimer(state.message_hwnd, crate::handlers::session::TIMER_SHELF);
    }
}

thread_local! {
    /// The card a Shelf context menu was opened on, for the command it posts.
    static MENU_TARGET: Cell<HWND> = const { Cell::new(null_mut()) };
    /// A card asked to close, re-checked by the close-verify timer.
    static CLOSING: Cell<HWND> = const { Cell::new(null_mut()) };
}

const ID_SHELF_SHOW: usize = 1;
const ID_SHELF_PIN: usize = 2;
const ID_SHELF_CLOSE: usize = 3;
/// Plus the side's index in `ShelfSide::ALL`.
const ID_SHELF_SIDE_BASE: usize = 10;

/// Right click on a card: its window's commands and the strip's position.
/// The strip owns the menu, so the menu always stacks above it and its
/// commands come back through `ShelfHost::command`.
pub(crate) fn show_shelf_menu(owner: HWND, card: HWND, at: POINT) {
    let Some(side) = crate::app::APP_STATE.with(|s| {
        let st = s.try_borrow().ok()?;
        Some(st.as_ref()?.config.shelf.side())
    }) else {
        return;
    };
    MENU_TARGET.with(|t| t.set(card));
    menu::show(owner, shelf_menu_entries(side), at);
}

fn shelf_menu_entries(current: ShelfSide) -> Vec<MenuEntry> {
    let item = |id, glyph, label: &str, submenu| {
        MenuEntry::Item(MenuItemData {
            id,
            glyph: Some(glyph),
            label: label.to_string(),
            shortcut: None,
            checked: false,
            submenu,
        })
    };
    let sides = ShelfSide::ALL
        .iter()
        .enumerate()
        .filter(|&(_, &side)| side != current)
        .map(|(i, &side)| {
            item(
                ID_SHELF_SIDE_BASE + i,
                shelf::side_glyph(side),
                shelf::side_label(side),
                None,
            )
        })
        .collect();
    vec![
        item(ID_SHELF_SHOW, GLYPH_RESTORE, t(Msg::ShelfMenuShow), None),
        item(ID_SHELF_PIN, GLYPH_PIN, t(Msg::ShelfMenuPin), None),
        MenuEntry::Separator,
        item(
            0,
            GLYPH_DOCK_LEFT,
            t(Msg::SettingsShelfSideTitle),
            Some(sides),
        ),
        MenuEntry::Separator,
        item(ID_SHELF_CLOSE, GLYPH_CLOSE, t(Msg::ShelfMenuClose), None),
    ]
}

pub(crate) fn on_shelf_menu_command(cmd: usize) {
    with_app_state(|state| on_shelf_command(state, cmd));
}

fn on_shelf_command(state: &mut AppState, cmd: usize) {
    let hwnd = MENU_TARGET.with(|t| t.replace(null_mut()));
    if let Some(&side) = cmd
        .checked_sub(ID_SHELF_SIDE_BASE)
        .and_then(|i| ShelfSide::ALL.get(i))
    {
        log_info!("Shelf menu: strip to the {} edge", side.tag());
        state.config.shelf.side = side.tag().to_string();
        crate::app::persist_config(&state.config);
        state.space_mgr.set_shelf_side(side);
        refresh_shelf(state);
        return;
    }
    if hwnd.is_null() {
        return;
    }
    match cmd {
        ID_SHELF_SHOW => state.space_mgr.shelf_promote(hwnd),
        ID_SHELF_PIN => state.space_mgr.set_sticky(hwnd, true),
        ID_SHELF_CLOSE => {
            // A "save changes?" answer would be owned by a hidden card; the
            // close-verify timer puts a window that survived on stage.
            CLOSING.with(|c| c.set(hwnd));
            crate::hostfns::request_close(state, hwnd);
        }
        _ => {}
    }
    refresh_shelf(state);
    if overview::is_overview_active() {
        overview::refresh_overview(&mut state.space_mgr);
    }
}

/// The close-verify tick: a card asked to close that is still alive is
/// waiting on the user (an unsaved-changes prompt), so bring it on stage.
pub(crate) fn after_shelf_close(state: &mut AppState) {
    let hwnd = CLOSING.with(|c| c.replace(null_mut()));
    if !hwnd.is_null() && winspaces_core::spaces::is_live_window(hwnd) {
        state.space_mgr.shelf_promote(hwnd);
        refresh_shelf(state);
    }
}

/// A window moved or resized: refresh Shelf when it is a stage that started
/// or stopped filling its monitor. Cheap for every other window.
pub(crate) fn on_window_moved(hwnd: HWND) {
    let changed = STAGES.with(|s| {
        s.borrow()
            .iter()
            .any(|&(h, filled)| h == hwnd && fills_monitor(h) != filled)
    });
    if changed {
        crate::schedule_shelf_timer();
    }
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

/// Single choke points for changing a monitor's space count: tray and Overview
/// both land here, so persistence, hotkey registration, the tray badge
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

/// Choke point for moving a space within a monitor — Overview's card
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
        if overview::is_overview_active() {
            overview::refresh_overview(&mut state.space_mgr);
        }
    }
}

pub(crate) fn after_space_count_change(state: &mut AppState, old_max: usize) {
    persist_space_structure(state);
    let new_max = state.space_mgr.max_space_count();
    if new_max != old_max && state.space_mgr.handle_hotkeys {
        HotkeyManager::unregister_all();
        if !HotkeyManager::register_all(&state.config, new_max) {
            log_warn!("Hotkey re-registration failed after space count change.");
        }
    }
    update_state_tray_icon(state);
    if overview::is_overview_active() {
        overview::refresh_overview(&mut state.space_mgr);
    }
}

/// Write the live per-monitor space counts and Shelf spaces straight into the
/// stored topology entry. The shadow path cannot be relied on for this:
/// `shadow_tick` refuses empty captures, so a change made with no windows open
/// would never reach disk. Cheap and user-initiated, so no debounce.
pub(crate) fn persist_space_structure(state: &mut AppState) {
    let signature = state.space_mgr.topology_signature();
    let live = layout_store::live_monitors(&state.space_mgr);
    layout_store::merge_space_structure(
        &mut state.layouts,
        state.shadow.as_mut(),
        &signature,
        &live,
    );
    if let Err(e) = state
        .layouts
        .save_to_file(&winspaces_common::LayoutStore::get_path())
    {
        log_error!(
            "Failed to save layouts.json after space count change: {}",
            e
        );
    }
}
