use crate::log_info;
use std::ptr::null_mut;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    RegisterHotKey, UnregisterHotKey, MOD_ALT, MOD_CONTROL, MOD_NOREPEAT, MOD_SHIFT,
};
use winspaces_common::{Config, MAX_DESKTOPS};

// The ID space is partitioned by the compile-time MAX, not the runtime count:
// switch 0..8, move 9..17, special 18+. IDs must never shift when the user
// adds or removes a space, or unregister_all would sweep the wrong IDs.
pub const HOTKEY_ID_SWITCH_BASE: i32 = 0;
pub const HOTKEY_ID_MOVE_BASE: i32 = MAX_DESKTOPS as i32;
pub const HOTKEY_ID_SPECIAL_BASE: i32 = (MAX_DESKTOPS * 2) as i32;

pub const HOTKEY_ID_EXIT: i32 = HOTKEY_ID_SPECIAL_BASE;
pub const HOTKEY_ID_TOGGLE: i32 = HOTKEY_ID_SPECIAL_BASE + 1;
pub const HOTKEY_ID_PREV: i32 = HOTKEY_ID_SPECIAL_BASE + 2;
pub const HOTKEY_ID_NEXT: i32 = HOTKEY_ID_SPECIAL_BASE + 3;
pub const HOTKEY_ID_MOVE_PREV: i32 = HOTKEY_ID_SPECIAL_BASE + 4;
pub const HOTKEY_ID_MOVE_NEXT: i32 = HOTKEY_ID_SPECIAL_BASE + 5;
pub const HOTKEY_ID_MISSION_CONTROL: i32 = HOTKEY_ID_SPECIAL_BASE + 6;

pub struct HotkeyManager;

impl HotkeyManager {
    /// `max_spaces` is the highest space count across monitors: digit hotkeys
    /// past it stay unregistered so Alt+5..9 aren't stolen from other apps
    /// while every monitor still has four spaces.
    pub fn register_all(config: &Config, max_spaces: usize) -> bool {
        log_info!("Registering global hotkeys...");
        Self::unregister_all();

        let mut registered: Vec<i32> = Vec::new();
        let mut ok = true;

        let mut attempt = |id: i32, mods: u32, vk: u32, ok: &mut bool| {
            if !*ok {
                return;
            }
            if Self::register(id, mods, vk) {
                log_info!("  [OK] hotkey id={} mods=0x{:X} vk=0x{:X}", id, mods, vk);
                registered.push(id);
            } else {
                log_info!("  [FAIL] hotkey id={} mods=0x{:X} vk=0x{:X}", id, mods, vk);
                *ok = false;
            }
        };

        for i in 0..max_spaces.min(MAX_DESKTOPS) {
            let hk = config.switch_desktops[i];
            if hk.vk != 0 {
                attempt(
                    HOTKEY_ID_SWITCH_BASE + i as i32,
                    hk.modifiers,
                    hk.vk,
                    &mut ok,
                );
            }
            let m_hk = config.move_desktops[i];
            if m_hk.vk != 0 {
                attempt(
                    HOTKEY_ID_MOVE_BASE + i as i32,
                    m_hk.modifiers,
                    m_hk.vk,
                    &mut ok,
                );
            }
        }
        attempt(
            HOTKEY_ID_EXIT,
            MOD_ALT | MOD_CONTROL | MOD_SHIFT,
            b'Q' as u32,
            &mut ok,
        );
        attempt(
            HOTKEY_ID_TOGGLE,
            MOD_ALT | MOD_CONTROL | MOD_SHIFT,
            b'S' as u32,
            &mut ok,
        );

        if config.prev.vk != 0 {
            attempt(
                HOTKEY_ID_PREV,
                config.prev.modifiers,
                config.prev.vk,
                &mut ok,
            );
        }
        if config.next.vk != 0 {
            attempt(
                HOTKEY_ID_NEXT,
                config.next.modifiers,
                config.next.vk,
                &mut ok,
            );
        }
        if config.move_prev.vk != 0 {
            attempt(
                HOTKEY_ID_MOVE_PREV,
                config.move_prev.modifiers,
                config.move_prev.vk,
                &mut ok,
            );
        }
        if config.move_next.vk != 0 {
            attempt(
                HOTKEY_ID_MOVE_NEXT,
                config.move_next.modifiers,
                config.move_next.vk,
                &mut ok,
            );
        }
        if config.mission_control.vk != 0 {
            attempt(
                HOTKEY_ID_MISSION_CONTROL,
                config.mission_control.modifiers,
                config.mission_control.vk,
                &mut ok,
            );
        }

        if !ok {
            log_info!("Hotkey registration had conflicts; rolling back all registrations.");
            for rid in &registered {
                unsafe {
                    UnregisterHotKey(null_mut(), *rid);
                }
            }
        }
        ok
    }

    pub fn unregister_all() {
        log_info!("Unregistering global hotkeys");
        unsafe {
            // Always sweep the full MAX range: after a count shrink the tail
            // IDs are still registered, and unregistering an unregistered ID
            // is a harmless no-op.
            for i in 0..MAX_DESKTOPS as i32 {
                UnregisterHotKey(null_mut(), HOTKEY_ID_SWITCH_BASE + i);
                UnregisterHotKey(null_mut(), HOTKEY_ID_MOVE_BASE + i);
            }
            UnregisterHotKey(null_mut(), HOTKEY_ID_EXIT);
            UnregisterHotKey(null_mut(), HOTKEY_ID_TOGGLE);
            UnregisterHotKey(null_mut(), HOTKEY_ID_PREV);
            UnregisterHotKey(null_mut(), HOTKEY_ID_NEXT);
            UnregisterHotKey(null_mut(), HOTKEY_ID_MOVE_PREV);
            UnregisterHotKey(null_mut(), HOTKEY_ID_MOVE_NEXT);
            UnregisterHotKey(null_mut(), HOTKEY_ID_MISSION_CONTROL);
        }
    }

    fn register(id: i32, modifiers: u32, vk: u32) -> bool {
        unsafe { RegisterHotKey(null_mut(), id, modifiers | MOD_NOREPEAT, vk) != 0 }
    }
}
