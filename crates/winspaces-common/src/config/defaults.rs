//! The serde defaults of every `Config` and `TilingConfig` field, and the
//! per-index defaults `normalize` pads short hotkey lists with. Each is a
//! named function so `#[serde(default = "...")]` can reach it: a config
//! written before a field existed must upgrade to the working binding, not
//! to a zeroed one.

use super::Hotkey;
use crate::spaces::MAX_SPACES;

pub(super) fn default_true() -> bool {
    true
}

/// Named so `#[serde(default = ...)]` reaches it: a bare `#[serde(default)]`
/// on a `String` yields `""`, which `normalize` would have to repair on every
/// pre-existing settings.json.
pub(super) fn default_language() -> String {
    crate::i18n::SYSTEM_TAG.to_string()
}

/// Default binding for "switch to space i+1": Alt+digit. Shared by
/// `Config::default` and `normalize`'s tail padding so a pre-existing short
/// config upgrades to working bindings instead of dead unassigned slots.
pub(super) fn default_switch_hotkey(i: usize) -> Hotkey {
    Hotkey {
        modifiers: 0x0001, // MOD_ALT
        vk: 0x31 + i as u32,
    }
}

/// Default binding for "move window to space i+1": Ctrl+Alt+digit.
pub(super) fn default_move_hotkey(i: usize) -> Hotkey {
    Hotkey {
        modifiers: 0x0001 | 0x0002, // MOD_ALT | MOD_CONTROL
        vk: 0x31 + i as u32,
    }
}

pub(super) fn default_switch_hotkeys() -> Vec<Hotkey> {
    (0..MAX_SPACES).map(default_switch_hotkey).collect()
}

pub(super) fn default_move_hotkeys() -> Vec<Hotkey> {
    (0..MAX_SPACES).map(default_move_hotkey).collect()
}

/// Ctrl+Up.
pub(super) fn default_overview_hotkey() -> Hotkey {
    Hotkey {
        modifiers: 0x0002, // MOD_CONTROL
        vk: 0x26,          // VK_UP
    }
}

/// Alt+Left.
pub(super) fn default_prev_hotkey() -> Hotkey {
    Hotkey {
        modifiers: 0x0001, // MOD_ALT
        vk: 0x25,          // VK_LEFT
    }
}

/// Alt+Right.
pub(super) fn default_next_hotkey() -> Hotkey {
    Hotkey {
        modifiers: 0x0001, // MOD_ALT
        vk: 0x27,          // VK_RIGHT
    }
}

/// Alt+Shift+Win+Left.
pub(super) fn default_move_prev_hotkey() -> Hotkey {
    Hotkey {
        modifiers: 0x0001 | 0x0004 | 0x0008, // MOD_ALT | MOD_SHIFT | MOD_WIN
        vk: 0x25,                            // VK_LEFT
    }
}

/// Alt+Shift+Win+Right.
pub(super) fn default_move_next_hotkey() -> Hotkey {
    Hotkey {
        modifiers: 0x0001 | 0x0004 | 0x0008, // MOD_ALT | MOD_SHIFT | MOD_WIN
        vk: 0x27,                            // VK_RIGHT
    }
}

/// Default binding for "pin the active window to every space": Ctrl+Alt+Shift+P.
///
/// In the four-modifier family the other whole-app actions use (taskbar mode is
/// Ctrl+Alt+Shift+S, exit is Ctrl+Alt+Shift+Q) rather than a two-modifier combo
/// a running app is likely to have claimed. That matters more here than it
/// looks: `HotkeyManager::register_all` rolls back *every* registration if any
/// single `RegisterHotKey` fails, so one collision costs the user all of their
/// WinSpaces hotkeys, not just this one.
///
/// Named rather than inlined so `#[serde(default = ...)]` can reach it: a
/// config written before this field existed must upgrade to the working
/// binding, exactly as the switch/move slots do, instead of deserializing to
/// `{0, 0}` and leaving long-time users the only ones without the hotkey.
pub(super) fn default_toggle_sticky_hotkey() -> Hotkey {
    Hotkey {
        modifiers: 0x0001 | 0x0002 | 0x0004, // MOD_ALT | MOD_CONTROL | MOD_SHIFT
        vk: 0x50,                            // VK_P
    }
}

/// Default binding for "toggle tiling on/off globally": Ctrl+Alt+Shift+T (0x7 / 0x54).
pub(super) fn default_tiling_toggle_hotkey() -> Hotkey {
    Hotkey {
        modifiers: 0x0001 | 0x0002 | 0x0004, // MOD_ALT | MOD_CONTROL | MOD_SHIFT
        vk: 0x54,                            // VK_T
    }
}

pub(super) fn default_tiling_focus_left_hotkey() -> Hotkey {
    Hotkey {
        modifiers: 0x0001 | 0x0002 | 0x0004, // MOD_ALT | MOD_CONTROL | MOD_SHIFT
        vk: 0x25,                            // VK_LEFT
    }
}

pub(super) fn default_tiling_focus_right_hotkey() -> Hotkey {
    Hotkey {
        modifiers: 0x0001 | 0x0002 | 0x0004, // MOD_ALT | MOD_CONTROL | MOD_SHIFT
        vk: 0x27,                            // VK_RIGHT
    }
}

pub(super) fn default_tiling_focus_up_hotkey() -> Hotkey {
    Hotkey {
        modifiers: 0x0001 | 0x0002 | 0x0004, // MOD_ALT | MOD_CONTROL | MOD_SHIFT
        vk: 0x26,                            // VK_UP
    }
}

pub(super) fn default_tiling_focus_down_hotkey() -> Hotkey {
    Hotkey {
        modifiers: 0x0001 | 0x0002 | 0x0004, // MOD_ALT | MOD_CONTROL | MOD_SHIFT
        vk: 0x28,                            // VK_DOWN
    }
}

pub(super) fn default_tiling_swap_left_hotkey() -> Hotkey {
    Hotkey {
        modifiers: 0x0002 | 0x0004 | 0x0008, // MOD_CONTROL | MOD_SHIFT | MOD_WIN
        vk: 0x25,                            // VK_LEFT
    }
}

pub(super) fn default_tiling_swap_right_hotkey() -> Hotkey {
    Hotkey {
        modifiers: 0x0002 | 0x0004 | 0x0008, // MOD_CONTROL | MOD_SHIFT | MOD_WIN
        vk: 0x27,                            // VK_RIGHT
    }
}

pub(super) fn default_tiling_swap_up_hotkey() -> Hotkey {
    Hotkey {
        modifiers: 0x0002 | 0x0004 | 0x0008, // MOD_CONTROL | MOD_SHIFT | MOD_WIN
        vk: 0x26,                            // VK_UP
    }
}

pub(super) fn default_tiling_swap_down_hotkey() -> Hotkey {
    Hotkey {
        modifiers: 0x0002 | 0x0004 | 0x0008, // MOD_CONTROL | MOD_SHIFT | MOD_WIN
        vk: 0x28,                            // VK_DOWN
    }
}

pub(super) fn default_tiling_ratio_shrink_hotkey() -> Hotkey {
    Hotkey {
        modifiers: 0x0001 | 0x0002 | 0x0004, // MOD_ALT | MOD_CONTROL | MOD_SHIFT
        vk: 0xBD,                            // VK_OEM_MINUS
    }
}

pub(super) fn default_tiling_ratio_grow_hotkey() -> Hotkey {
    Hotkey {
        modifiers: 0x0001 | 0x0002 | 0x0004, // MOD_ALT | MOD_CONTROL | MOD_SHIFT
        vk: 0xBB,                            // VK_OEM_PLUS
    }
}

pub(super) fn default_tiling_toggle_float_hotkey() -> Hotkey {
    Hotkey {
        modifiers: 0x0001 | 0x0002 | 0x0004, // MOD_ALT | MOD_CONTROL | MOD_SHIFT
        vk: 0x46,                            // VK_F
    }
}

pub(super) fn default_tiling_toggle_split_hotkey() -> Hotkey {
    Hotkey {
        modifiers: 0x0001 | 0x0002 | 0x0004, // MOD_ALT | MOD_CONTROL | MOD_SHIFT
        vk: 0x4F,                            // VK_O
    }
}

pub(super) fn default_ratio_step_pct() -> u32 {
    5
}
