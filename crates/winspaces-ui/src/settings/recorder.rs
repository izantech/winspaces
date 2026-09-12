//! Hotkey capture for the settings window.
//!
//! While a hotkey field is recording, a temporary `WH_KEYBOARD_LL` hook owns
//! the keyboard: it sees keys *before* `RegisterHotKey` consumers (the running
//! daemon) and before the shell, so combos that are already live global
//! hotkeys — and bare `Win+X` combos — are capturable. The callback does the
//! minimum allowed inside an LL hook (same discipline as
//! `low_level_keyboard_proc` in main.rs): post the virtual key to the
//! settings window and return.
//!
//! The pure `translate` step (modifier snapshot + VK -> outcome) is separated
//! from the hook plumbing so it can be unit tested.

use std::cell::Cell;
use std::ptr::null_mut;
use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    keybd_event, GetAsyncKeyState, KEYEVENTF_KEYUP, VK_CONTROL, VK_ESCAPE, VK_LCONTROL, VK_LMENU,
    VK_LSHIFT, VK_LWIN, VK_MENU, VK_RCONTROL, VK_RMENU, VK_RSHIFT, VK_RWIN, VK_SHIFT,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, SetWindowsHookExW, UnhookWindowsHookEx, HHOOK, KBDLLHOOKSTRUCT, WH_KEYBOARD_LL,
    WM_KEYDOWN, WM_SYSKEYDOWN,
};
use winspaces_common::Hotkey;

/// Posted to the settings window for every swallowed non-modifier keydown.
/// `wparam` carries the virtual key.
pub const WM_APP_RECORDER_KEY: u32 = windows_sys::Win32::UI::WindowsAndMessaging::WM_APP + 70;

pub const MOD_ALT: u32 = 0x0001;
pub const MOD_CONTROL: u32 = 0x0002;
pub const MOD_SHIFT: u32 = 0x0004;
pub const MOD_WIN: u32 = 0x0008;

#[derive(Debug, PartialEq, Eq)]
pub enum Outcome {
    /// Bare modifier press: stay in capture mode.
    Ignore,
    /// Esc pressed: leave capture mode, keep the old binding.
    Cancel,
    /// A complete combo was pressed.
    Commit(Hotkey),
}

pub fn is_modifier_vk(vk: u32) -> bool {
    matches!(
        vk as u16,
        x if x == VK_CONTROL
            || x == VK_LCONTROL
            || x == VK_RCONTROL
            || x == VK_MENU
            || x == VK_LMENU
            || x == VK_RMENU
            || x == VK_SHIFT
            || x == VK_LSHIFT
            || x == VK_RSHIFT
            || x == VK_LWIN
            || x == VK_RWIN
    )
}

/// Pure translation of a captured keydown plus the modifier snapshot taken at
/// that moment. `Hotkey.modifiers` uses the `RegisterHotKey` MOD_* bit layout
/// the config file already stores (masked to 0x0F by `Config::normalize`).
pub fn translate(vk: u32, ctrl: bool, alt: bool, shift: bool, win: bool) -> Outcome {
    if vk as u16 == VK_ESCAPE {
        return Outcome::Cancel;
    }
    if is_modifier_vk(vk) {
        return Outcome::Ignore;
    }
    let mut modifiers = 0u32;
    if ctrl {
        modifiers |= MOD_CONTROL;
    }
    if alt {
        modifiers |= MOD_ALT;
    }
    if shift {
        modifiers |= MOD_SHIFT;
    }
    if win {
        modifiers |= MOD_WIN;
    }
    Outcome::Commit(Hotkey { modifiers, vk })
}

/// Snapshot the live modifier state and translate a captured keydown.
pub fn translate_current(vk: u32) -> Outcome {
    let down = |k: u16| unsafe { GetAsyncKeyState(k as i32) as u16 & 0x8000 != 0 };
    translate(
        vk,
        down(VK_CONTROL),
        down(VK_MENU),
        down(VK_SHIFT),
        down(VK_LWIN) || down(VK_RWIN),
    )
}

thread_local! {
    static HOOK: Cell<HHOOK> = const { Cell::new(null_mut()) };
    static TARGET: Cell<HWND> = const { Cell::new(null_mut()) };
}

unsafe extern "system" fn recorder_ll_proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code >= 0 && (wparam == WM_KEYDOWN as usize || wparam == WM_SYSKEYDOWN as usize) {
        let kbd = &*(lparam as *const KBDLLHOOKSTRUCT);
        let vk = kbd.vkCode;
        // Modifier keydowns pass through so the async key state (and other
        // apps' view of the modifiers) stays accurate; everything else is
        // swallowed and re-posted to the settings window.
        if !is_modifier_vk(vk) {
            let target = TARGET.with(|t| t.get());
            if !target.is_null() {
                windows_sys::Win32::UI::WindowsAndMessaging::PostMessageW(
                    target,
                    WM_APP_RECORDER_KEY,
                    vk as usize,
                    0,
                );
                // If Win is held, inject a no-op key so the swallowed key
                // still counts as "a key was pressed while Win was down" —
                // otherwise the Start menu opens on Win release (same trick
                // as the daemon's Win+Tab interception).
                let win_down = GetAsyncKeyState(VK_LWIN as i32) as u16 & 0x8000 != 0
                    || GetAsyncKeyState(VK_RWIN as i32) as u16 & 0x8000 != 0;
                if win_down {
                    keybd_event(0xFF, 0, 0, 0);
                    keybd_event(0xFF, 0, KEYEVENTF_KEYUP, 0);
                }
                return 1;
            }
        }
    }
    CallNextHookEx(null_mut(), code, wparam, lparam)
}

/// Install the capture hook targeting `hwnd`. Returns false when the hook
/// could not be installed; the caller then falls back to plain WM_KEYDOWN
/// capture.
pub fn start_capture(hwnd: HWND) -> bool {
    stop_capture();
    unsafe {
        let hook = SetWindowsHookExW(
            WH_KEYBOARD_LL,
            Some(recorder_ll_proc),
            windows_sys::Win32::System::LibraryLoader::GetModuleHandleW(null_mut()),
            0,
        );
        if hook.is_null() {
            return false;
        }
        TARGET.with(|t| t.set(hwnd));
        HOOK.with(|h| h.set(hook));
        true
    }
}

/// Uninstall the capture hook. Safe to call when no capture is active.
pub fn stop_capture() {
    let hook = HOOK.with(|h| h.replace(null_mut()));
    TARGET.with(|t| t.set(null_mut()));
    if !hook.is_null() {
        unsafe {
            UnhookWindowsHookEx(hook);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn commit_carries_all_modifiers() {
        assert_eq!(
            translate(0x41, true, true, true, true),
            Outcome::Commit(Hotkey {
                modifiers: MOD_CONTROL | MOD_ALT | MOD_SHIFT | MOD_WIN,
                vk: 0x41
            })
        );
    }

    #[test]
    fn commit_without_modifiers_is_allowed() {
        // A bare key (e.g. F5) is a valid binding.
        assert_eq!(
            translate(0x74, false, false, false, false),
            Outcome::Commit(Hotkey {
                modifiers: 0,
                vk: 0x74
            })
        );
    }

    #[test]
    fn bare_modifiers_are_ignored() {
        for vk in [
            VK_CONTROL,
            VK_LCONTROL,
            VK_RCONTROL,
            VK_MENU,
            VK_LMENU,
            VK_RMENU,
            VK_SHIFT,
            VK_LSHIFT,
            VK_RSHIFT,
            VK_LWIN,
            VK_RWIN,
        ] {
            assert_eq!(
                translate(vk as u32, true, false, false, false),
                Outcome::Ignore
            );
        }
    }

    #[test]
    fn escape_cancels_even_with_modifiers_down() {
        assert_eq!(
            translate(VK_ESCAPE as u32, true, true, false, false),
            Outcome::Cancel
        );
    }

    #[test]
    fn committed_hotkey_survives_config_normalization() {
        // Round-trip: a recorded hotkey written into the config keeps its
        // exact shape through normalize() (the masked bits are already the
        // only ones the recorder can produce).
        let Outcome::Commit(hk) = translate(0x31, true, true, false, true) else {
            panic!("expected commit");
        };
        let mut cfg = winspaces_common::Config::default();
        cfg.switch_spaces[0] = hk;
        cfg.normalize();
        assert_eq!(cfg.switch_spaces[0], hk);
    }

    #[test]
    fn recorded_hotkey_round_trips_through_json() {
        let Outcome::Commit(hk) = translate(0x56, false, true, true, false) else {
            panic!("expected commit");
        };
        let cfg = winspaces_common::Config {
            overview: hk,
            ..Default::default()
        };
        let json = serde_json::to_string(&cfg).unwrap();
        let back: winspaces_common::Config = serde_json::from_str(&json).unwrap();
        assert_eq!(back.overview, hk);
    }
}
