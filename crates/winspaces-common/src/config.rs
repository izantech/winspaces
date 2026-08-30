use crate::paths::{config_dir, write_json_atomic};
use crate::spaces::MAX_SPACES;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Hotkey {
    pub modifiers: u32,
    pub vk: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct WindowRect {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
}

impl WindowRect {
    pub fn width(&self) -> i32 {
        self.right - self.left
    }

    pub fn height(&self) -> i32 {
        self.bottom - self.top
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WorkspaceRule {
    pub name: String,
    #[serde(default)]
    pub aumid: String,
    pub exe_path: String,
    pub class_name: String,
    pub title_pattern: String,
    pub display_index: usize,
    pub space_index: usize,
    pub show_cmd: u32,
    pub rect: WindowRect,
    #[serde(default)]
    pub is_snapped: bool,
    #[serde(default)]
    pub is_sticky: bool,
}

impl Default for WorkspaceRule {
    fn default() -> Self {
        Self {
            name: "New Rule".to_string(),
            aumid: String::new(),
            exe_path: String::new(),
            class_name: String::new(),
            title_pattern: String::new(),
            display_index: 0,
            space_index: 0,
            show_cmd: 1,
            rect: WindowRect::default(),
            is_snapped: false,
            is_sticky: false,
        }
    }
}

fn default_true() -> bool {
    true
}

/// Named so `#[serde(default = ...)]` reaches it: a bare `#[serde(default)]`
/// on a `String` yields `""`, which `normalize` would have to repair on every
/// pre-existing settings.json.
fn default_language() -> String {
    crate::i18n::SYSTEM_TAG.to_string()
}

/// Default binding for "switch to space i+1": Alt+digit. Shared by
/// `Config::default` and `normalize`'s tail padding so a pre-existing short
/// config upgrades to working bindings instead of dead unassigned slots.
fn default_switch_hotkey(i: usize) -> Hotkey {
    Hotkey {
        modifiers: 0x0001, // MOD_ALT
        vk: 0x31 + i as u32,
    }
}

/// Default binding for "move window to space i+1": Ctrl+Alt+digit.
fn default_move_hotkey(i: usize) -> Hotkey {
    Hotkey {
        modifiers: 0x0001 | 0x0002, // MOD_ALT | MOD_CONTROL
        vk: 0x31 + i as u32,
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
fn default_toggle_sticky_hotkey() -> Hotkey {
    Hotkey {
        modifiers: 0x0001 | 0x0002 | 0x0004, // MOD_ALT | MOD_CONTROL | MOD_SHIFT
        vk: 0x50,                            // VK_P
    }
}

/// Default binding for "toggle tiling on/off globally": Ctrl+Alt+Shift+T (0x7 / 0x54).
fn default_tiling_toggle_hotkey() -> Hotkey {
    Hotkey {
        modifiers: 0x0001 | 0x0002 | 0x0004, // MOD_ALT | MOD_CONTROL | MOD_SHIFT
        vk: 0x54,                            // VK_T
    }
}

fn default_tiling_focus_left_hotkey() -> Hotkey {
    Hotkey {
        modifiers: 0x0001 | 0x0002 | 0x0004, // MOD_ALT | MOD_CONTROL | MOD_SHIFT
        vk: 0x25,                            // VK_LEFT
    }
}

fn default_tiling_focus_right_hotkey() -> Hotkey {
    Hotkey {
        modifiers: 0x0001 | 0x0002 | 0x0004, // MOD_ALT | MOD_CONTROL | MOD_SHIFT
        vk: 0x27,                            // VK_RIGHT
    }
}

fn default_tiling_focus_up_hotkey() -> Hotkey {
    Hotkey {
        modifiers: 0x0001 | 0x0002 | 0x0004, // MOD_ALT | MOD_CONTROL | MOD_SHIFT
        vk: 0x26,                            // VK_UP
    }
}

fn default_tiling_focus_down_hotkey() -> Hotkey {
    Hotkey {
        modifiers: 0x0001 | 0x0002 | 0x0004, // MOD_ALT | MOD_CONTROL | MOD_SHIFT
        vk: 0x28,                            // VK_DOWN
    }
}

fn default_tiling_swap_left_hotkey() -> Hotkey {
    Hotkey {
        modifiers: 0x0002 | 0x0004 | 0x0008, // MOD_CONTROL | MOD_SHIFT | MOD_WIN
        vk: 0x25,                            // VK_LEFT
    }
}

fn default_tiling_swap_right_hotkey() -> Hotkey {
    Hotkey {
        modifiers: 0x0002 | 0x0004 | 0x0008, // MOD_CONTROL | MOD_SHIFT | MOD_WIN
        vk: 0x27,                            // VK_RIGHT
    }
}

fn default_tiling_swap_up_hotkey() -> Hotkey {
    Hotkey {
        modifiers: 0x0002 | 0x0004 | 0x0008, // MOD_CONTROL | MOD_SHIFT | MOD_WIN
        vk: 0x26,                            // VK_UP
    }
}

fn default_tiling_swap_down_hotkey() -> Hotkey {
    Hotkey {
        modifiers: 0x0002 | 0x0004 | 0x0008, // MOD_CONTROL | MOD_SHIFT | MOD_WIN
        vk: 0x28,                            // VK_DOWN
    }
}

fn default_tiling_ratio_shrink_hotkey() -> Hotkey {
    Hotkey {
        modifiers: 0x0001 | 0x0002 | 0x0004, // MOD_ALT | MOD_CONTROL | MOD_SHIFT
        vk: 0xBD,                            // VK_OEM_MINUS
    }
}

fn default_tiling_ratio_grow_hotkey() -> Hotkey {
    Hotkey {
        modifiers: 0x0001 | 0x0002 | 0x0004, // MOD_ALT | MOD_CONTROL | MOD_SHIFT
        vk: 0xBB,                            // VK_OEM_PLUS
    }
}

fn default_tiling_toggle_float_hotkey() -> Hotkey {
    Hotkey {
        modifiers: 0x0001 | 0x0002 | 0x0004, // MOD_ALT | MOD_CONTROL | MOD_SHIFT
        vk: 0x46,                            // VK_F
    }
}

fn default_tiling_toggle_split_hotkey() -> Hotkey {
    Hotkey {
        modifiers: 0x0001 | 0x0002 | 0x0004, // MOD_ALT | MOD_CONTROL | MOD_SHIFT
        vk: 0x4F,                            // VK_O
    }
}

fn default_ratio_step_pct() -> u32 {
    5
}

/// Dynamic tiling configuration.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct FloatRule {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub aumid: String,
    #[serde(default)]
    pub exe_path: String,
    #[serde(default)]
    pub class_name: String,
    #[serde(default)]
    pub title_pattern: String,
}

impl FloatRule {
    /// Adapts this `FloatRule` to a `WorkspaceRule` so the pure `score_rule` matcher
    /// can evaluate specificity without duplicating matching logic.
    pub fn as_workspace_rule(&self) -> WorkspaceRule {
        WorkspaceRule {
            name: self.name.clone(),
            aumid: self.aumid.clone(),
            exe_path: self.exe_path.clone(),
            class_name: self.class_name.clone(),
            title_pattern: self.title_pattern.clone(),
            ..Default::default()
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TilingConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub inner_gap: u32,
    #[serde(default)]
    pub outer_gap: u32,
    #[serde(default = "default_ratio_step_pct")]
    pub ratio_step_pct: u32,
    #[serde(default = "default_tiling_toggle_hotkey")]
    pub toggle: Hotkey,
    #[serde(default = "default_tiling_focus_left_hotkey")]
    pub focus_left: Hotkey,
    #[serde(default = "default_tiling_focus_right_hotkey")]
    pub focus_right: Hotkey,
    #[serde(default = "default_tiling_focus_up_hotkey")]
    pub focus_up: Hotkey,
    #[serde(default = "default_tiling_focus_down_hotkey")]
    pub focus_down: Hotkey,
    #[serde(default = "default_tiling_swap_left_hotkey")]
    pub swap_left: Hotkey,
    #[serde(default = "default_tiling_swap_right_hotkey")]
    pub swap_right: Hotkey,
    #[serde(default = "default_tiling_swap_up_hotkey")]
    pub swap_up: Hotkey,
    #[serde(default = "default_tiling_swap_down_hotkey")]
    pub swap_down: Hotkey,
    #[serde(default = "default_tiling_ratio_shrink_hotkey")]
    pub ratio_shrink: Hotkey,
    #[serde(default = "default_tiling_ratio_grow_hotkey")]
    pub ratio_grow: Hotkey,
    #[serde(default = "default_tiling_toggle_float_hotkey")]
    pub toggle_float: Hotkey,
    #[serde(default = "default_tiling_toggle_split_hotkey")]
    pub toggle_split: Hotkey,
    #[serde(default)]
    pub float_rules: Vec<FloatRule>,
}

impl Default for TilingConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            inner_gap: 0,
            outer_gap: 0,
            ratio_step_pct: default_ratio_step_pct(),
            toggle: default_tiling_toggle_hotkey(),
            focus_left: default_tiling_focus_left_hotkey(),
            focus_right: default_tiling_focus_right_hotkey(),
            focus_up: default_tiling_focus_up_hotkey(),
            focus_down: default_tiling_focus_down_hotkey(),
            swap_left: default_tiling_swap_left_hotkey(),
            swap_right: default_tiling_swap_right_hotkey(),
            swap_up: default_tiling_swap_up_hotkey(),
            swap_down: default_tiling_swap_down_hotkey(),
            ratio_shrink: default_tiling_ratio_shrink_hotkey(),
            ratio_grow: default_tiling_ratio_grow_hotkey(),
            toggle_float: default_tiling_toggle_float_hotkey(),
            toggle_split: default_tiling_toggle_split_hotkey(),
            float_rules: Vec::new(),
        }
    }
}

impl TilingConfig {
    pub fn normalize(&mut self) {
        self.ratio_step_pct = self.ratio_step_pct.clamp(1, 50);
        self.inner_gap = self.inner_gap.min(256);
        self.outer_gap = self.outer_gap.min(256);
        self.sanitize_modifiers();
    }

    pub fn sanitize_modifiers(&mut self) {
        const MASK: u32 = 0x0001 | 0x0002 | 0x0004 | 0x0008;
        self.toggle.modifiers &= MASK;
        self.focus_left.modifiers &= MASK;
        self.focus_right.modifiers &= MASK;
        self.focus_up.modifiers &= MASK;
        self.focus_down.modifiers &= MASK;
        self.swap_left.modifiers &= MASK;
        self.swap_right.modifiers &= MASK;
        self.swap_up.modifiers &= MASK;
        self.swap_down.modifiers &= MASK;
        self.ratio_shrink.modifiers &= MASK;
        self.ratio_grow.modifiers &= MASK;
        self.toggle_float.modifiers &= MASK;
        self.toggle_split.modifiers &= MASK;
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Config {
    /// UI language: `"system"` follows the Windows display language; a
    /// locale tag with a `locales/<tag>.json` (`"en"`, `"es"`) pins it.
    /// Anything else normalizes back to `"system"`.
    #[serde(default = "default_language")]
    pub language: String,
    pub show_all_taskbar: bool,
    #[serde(default)]
    pub auto_restore_workspaces: bool,
    #[serde(default = "default_true")]
    pub intercept_win_tab: bool,
    /// Show the transient "Space N" indicator on the monitor whose space
    /// just changed.
    #[serde(default = "default_true")]
    pub space_indicator: bool,
    #[serde(default)]
    pub mission_control: Hotkey,
    pub switch_spaces: Vec<Hotkey>,
    pub move_spaces: Vec<Hotkey>,
    pub prev: Hotkey,
    pub next: Hotkey,
    pub move_prev: Hotkey,
    pub move_next: Hotkey,
    #[serde(default = "default_toggle_sticky_hotkey")]
    pub toggle_sticky: Hotkey,
    #[serde(default)]
    pub workspace_rules: Vec<WorkspaceRule>,
    #[serde(default)]
    pub tiling: TilingConfig,
}

impl Default for Config {
    fn default() -> Self {
        const MOD_ALT: u32 = 0x0001;
        const MOD_CONTROL: u32 = 0x0002;
        const MOD_SHIFT: u32 = 0x0004;
        const MOD_WIN: u32 = 0x0008;

        const VK_LEFT: u32 = 0x25;
        const VK_UP: u32 = 0x26;
        const VK_RIGHT: u32 = 0x27;

        let switch_spaces: Vec<Hotkey> = (0..MAX_SPACES).map(default_switch_hotkey).collect();
        let move_spaces: Vec<Hotkey> = (0..MAX_SPACES).map(default_move_hotkey).collect();

        Self {
            language: default_language(),
            show_all_taskbar: false,
            auto_restore_workspaces: false,
            intercept_win_tab: true,
            space_indicator: true,
            mission_control: Hotkey {
                modifiers: MOD_CONTROL,
                vk: VK_UP,
            },
            switch_spaces,
            move_spaces,
            prev: Hotkey {
                modifiers: MOD_ALT,
                vk: VK_LEFT,
            },
            next: Hotkey {
                modifiers: MOD_ALT,
                vk: VK_RIGHT,
            },
            move_prev: Hotkey {
                modifiers: MOD_ALT | MOD_SHIFT | MOD_WIN,
                vk: VK_LEFT,
            },
            move_next: Hotkey {
                modifiers: MOD_ALT | MOD_SHIFT | MOD_WIN,
                vk: VK_RIGHT,
            },
            toggle_sticky: default_toggle_sticky_hotkey(),
            workspace_rules: Vec::new(),
            tiling: TilingConfig::default(),
        }
    }
}

impl Config {
    pub fn get_config_path() -> PathBuf {
        config_dir().join("settings.json")
    }

    pub fn load_from_file(path: &Path) -> Self {
        let content = match fs::read_to_string(path) {
            Ok(c) => c,
            Err(_) => {
                let default_cfg = Self::default();
                let _ = default_cfg.save_to_file(path);
                return default_cfg;
            }
        };

        match serde_json::from_str::<Config>(&content) {
            Ok(mut cfg) => {
                cfg.normalize();
                cfg
            }
            Err(_) => {
                // Preserve the unparseable file instead of destroying the user's
                // settings (and any captured workspace rules) on a hand-edit typo.
                let backup = path.with_extension("json.bak");
                let _ = fs::rename(path, &backup);
                let default_cfg = Self::default();
                let _ = default_cfg.save_to_file(path);
                default_cfg
            }
        }
    }

    pub fn save_to_file(&self, path: &Path) -> std::io::Result<()> {
        write_json_atomic(path, self)
    }

    /// Read a config the user picked off disk. Deliberately *not*
    /// `load_from_file`: that one owns `settings.json` and answers a bad file
    /// by renaming it aside and installing defaults. An import must never
    /// touch the file it was handed, and "this backup is not readable" has to
    /// reach the user instead of silently resetting their settings — so the
    /// error comes back as a message to show.
    pub fn import_from_file(path: &Path) -> Result<Self, String> {
        let content = fs::read_to_string(path).map_err(|e| e.to_string())?;
        let mut cfg = serde_json::from_str::<Config>(&content).map_err(|e| e.to_string())?;
        cfg.normalize();
        Ok(cfg)
    }

    /// Repair any config shape the daemon cannot safely consume. GUIs and
    /// hand-edits may produce short or oversized hotkey lists; hotkey
    /// registration indexes `switch_spaces[0..MAX_SPACES]` directly.
    ///
    /// Short lists are tail-padded with the per-index *defaults* rather than
    /// unassigned entries: a settings.json written when there were only four
    /// spaces upgrades to working Alt+5..9 bindings. Explicit `vk: 0`
    /// entries inside the stored length are the user's choice and survive.
    pub fn normalize(&mut self) {
        self.language = match crate::i18n::Lang::from_tag(&self.language) {
            Some(lang) => lang.tag().to_string(),
            None => default_language(),
        };
        self.switch_spaces.truncate(MAX_SPACES);
        for i in self.switch_spaces.len()..MAX_SPACES {
            self.switch_spaces.push(default_switch_hotkey(i));
        }
        self.move_spaces.truncate(MAX_SPACES);
        for i in self.move_spaces.len()..MAX_SPACES {
            self.move_spaces.push(default_move_hotkey(i));
        }
        self.tiling.normalize();
        self.sanitize_modifiers();
    }

    pub fn sanitize_modifiers(&mut self) {
        const MASK: u32 = 0x0001 | 0x0002 | 0x0004 | 0x0008;
        for hk in self
            .switch_spaces
            .iter_mut()
            .chain(self.move_spaces.iter_mut())
        {
            hk.modifiers &= MASK;
        }
        self.prev.modifiers &= MASK;
        self.next.modifiers &= MASK;
        self.move_prev.modifiers &= MASK;
        self.move_next.modifiers &= MASK;
        self.mission_control.modifiers &= MASK;
        self.toggle_sticky.modifiers &= MASK;
        self.tiling.sanitize_modifiers();
    }
}

/// Display form of a hotkey in the current UI language: `Ctrl+Alt+1`,
/// `Ctrl+Mayús+Supr`. Display only — config persists `modifiers`/`vk`
/// numerically and nothing parses this back.
pub fn hotkey_to_string(hk: &Hotkey) -> String {
    hotkey_to_string_in(crate::i18n::current(), hk)
}

/// `Msg` for the keys that have a name rather than a legend. Letters,
/// digits, F-keys and punctuation are the same on every keyboard and stay
/// untranslated.
fn key_msg(vk: u32) -> Option<crate::i18n::Msg> {
    use crate::i18n::Msg;
    Some(match vk {
        0x09 => Msg::KeyTab,
        0x1B => Msg::KeyEsc,
        0x20 => Msg::KeySpace,
        0x0D => Msg::KeyEnter,
        0x08 => Msg::KeyBackspace,
        0x2E => Msg::KeyDelete,
        0x24 => Msg::KeyHome,
        0x23 => Msg::KeyEnd,
        0x21 => Msg::KeyPageUp,
        0x22 => Msg::KeyPageDown,
        0x25 => Msg::KeyLeft,
        0x27 => Msg::KeyRight,
        0x26 => Msg::KeyUp,
        0x28 => Msg::KeyDown,
        _ => return None,
    })
}

pub fn hotkey_to_string_in(lang: crate::i18n::Lang, hk: &Hotkey) -> String {
    use crate::i18n::{t_in, Msg};
    if hk.vk == 0 {
        return t_in(lang, Msg::KeyUnassigned).to_string();
    }
    let mut parts: Vec<&str> = Vec::with_capacity(5);
    if (hk.modifiers & 0x0002) != 0 {
        parts.push(t_in(lang, Msg::KeyCtrl));
    }
    if (hk.modifiers & 0x0001) != 0 {
        parts.push(t_in(lang, Msg::KeyAlt));
    }
    if (hk.modifiers & 0x0004) != 0 {
        parts.push(t_in(lang, Msg::KeyShift));
    }
    if (hk.modifiers & 0x0008) != 0 {
        parts.push(t_in(lang, Msg::KeyWin));
    }

    let vk = hk.vk;
    let vk_str: String = if (0x41..=0x5A).contains(&vk) || (0x30..=0x39).contains(&vk) {
        char::from_u32(vk)
            .map(|c| c.to_string())
            .unwrap_or_default()
    } else if (0x70..=0x87).contains(&vk) {
        format!("F{}", vk - 0x70 + 1)
    } else if let Some(msg) = key_msg(vk) {
        t_in(lang, msg).to_string()
    } else {
        match vk {
            0xBB => "+".to_string(),
            0xBD => "-".to_string(),
            0xBC => ",".to_string(),
            0xBE => ".".to_string(),
            0xBA => ";".to_string(),
            0xBF => "/".to_string(),
            0xC0 => "`".to_string(),
            0xDB => "[".to_string(),
            0xDD => "]".to_string(),
            0xDC => "\\".to_string(),
            0xDE => "'".to_string(),
            _ => format!("VK{}", vk),
        }
    };
    parts.push(&vk_str);
    parts.join("+")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_pads_short_hotkey_lists_with_defaults() {
        // A 4-entry config from before dynamic spaces upgrades to working
        // Alt+5..9 bindings, not dead unassigned slots.
        let mut cfg = Config::default();
        cfg.switch_spaces.truncate(4);
        cfg.move_spaces.truncate(1);
        cfg.normalize();
        assert_eq!(cfg.switch_spaces.len(), MAX_SPACES);
        assert_eq!(cfg.move_spaces.len(), MAX_SPACES);
        // Index 4 pads to Alt+5 (0x35), not Hotkey::default().
        assert_eq!(cfg.switch_spaces[4], default_switch_hotkey(4));
        assert_eq!(cfg.switch_spaces[4].vk, 0x35);
        assert_eq!(cfg.move_spaces[8], default_move_hotkey(8));
    }

    #[test]
    fn normalize_preserves_explicit_unassigned_entries() {
        // vk: 0 inside the stored length is a deliberate unbinding; only the
        // missing tail gets defaults.
        let mut cfg = Config::default();
        cfg.switch_spaces[2] = Hotkey::default();
        cfg.normalize();
        assert_eq!(cfg.switch_spaces[2], Hotkey::default());
    }

    #[test]
    fn normalize_truncates_oversized_hotkey_lists() {
        let mut cfg = Config::default();
        cfg.switch_spaces
            .extend(std::iter::repeat_n(Hotkey::default(), 10));
        cfg.normalize();
        assert_eq!(cfg.switch_spaces.len(), MAX_SPACES);
    }

    #[test]
    fn normalize_masks_unknown_modifier_bits() {
        let mut cfg = Config::default();
        cfg.prev.modifiers = 0xFFFF_FFFF;
        cfg.normalize();
        assert_eq!(cfg.prev.modifiers, 0x000F);
    }

    #[test]
    fn empty_hotkey_lists_deserialize_and_normalize() {
        // Hand-edited or truncated config shape: present but empty hotkey
        // arrays. Must never panic downstream.
        let json = r#"{
            "show_all_taskbar": false,
            "switch_spaces": [],
            "move_spaces": [],
            "prev": {"modifiers": 0, "vk": 0},
            "next": {"modifiers": 0, "vk": 0},
            "move_prev": {"modifiers": 0, "vk": 0},
            "move_next": {"modifiers": 0, "vk": 0}
        }"#;
        let mut cfg: Config = serde_json::from_str(json).unwrap();
        cfg.normalize();
        assert_eq!(cfg.switch_spaces.len(), MAX_SPACES);
    }

    #[test]
    fn config_deserializes_and_normalizes_space_fields() {
        let json = r#"{
            "show_all_taskbar": false,
            "switch_spaces": [{"modifiers": 1, "vk": 49}],
            "move_spaces": [{"modifiers": 3, "vk": 49}],
            "prev": {"modifiers": 0, "vk": 0},
            "next": {"modifiers": 0, "vk": 0},
            "move_prev": {"modifiers": 0, "vk": 0},
            "move_next": {"modifiers": 0, "vk": 0},
            "workspace_rules": [{
                "name": "Test App",
                "exe_path": "C:\\test.exe",
                "class_name": "TestClass",
                "title_pattern": "Test",
                "display_index": 0,
                "space_index": 2,
                "show_cmd": 1,
                "rect": {"left": 0, "top": 0, "right": 100, "bottom": 100}
            }]
        }"#;
        let mut cfg: Config = serde_json::from_str(json).unwrap();
        cfg.normalize();
        assert_eq!(cfg.switch_spaces.len(), MAX_SPACES);
        assert_eq!(
            cfg.switch_spaces[0],
            Hotkey {
                modifiers: 1,
                vk: 49
            }
        );
        assert_eq!(
            cfg.move_spaces[0],
            Hotkey {
                modifiers: 3,
                vk: 49
            }
        );
        assert_eq!(cfg.workspace_rules.len(), 1);
        assert_eq!(cfg.workspace_rules[0].space_index, 2);
        assert!(!cfg.workspace_rules[0].is_sticky);
        // Written before sticky windows existed: the field must upgrade to the
        // working default, not to a dead `{0, 0}` that leaves exactly the
        // long-time users without the hotkey a fresh install ships with.
        assert_eq!(cfg.toggle_sticky, Config::default().toggle_sticky);
        assert_ne!(cfg.toggle_sticky.vk, 0);
    }

    #[test]
    fn config_deserializes_sticky_rule_and_toggle_sticky_hotkey() {
        let json = r#"{
            "show_all_taskbar": false,
            "switch_spaces": [],
            "move_spaces": [],
            "prev": {"modifiers": 0, "vk": 0},
            "next": {"modifiers": 0, "vk": 0},
            "move_prev": {"modifiers": 0, "vk": 0},
            "move_next": {"modifiers": 0, "vk": 0},
            "toggle_sticky": {"modifiers": 5, "vk": 83},
            "workspace_rules": [{
                "name": "Sticky App",
                "exe_path": "C:\\sticky.exe",
                "class_name": "StickyClass",
                "title_pattern": "Sticky",
                "display_index": 0,
                "space_index": 0,
                "show_cmd": 1,
                "rect": {"left": 0, "top": 0, "right": 100, "bottom": 100},
                "is_sticky": true
            }]
        }"#;
        let mut cfg: Config = serde_json::from_str(json).unwrap();
        cfg.normalize();
        assert_eq!(
            cfg.toggle_sticky,
            Hotkey {
                modifiers: 5,
                vk: 83
            }
        );
        assert!(cfg.workspace_rules[0].is_sticky);
    }

    #[test]
    fn hotkey_to_string_formats_known_keys() {
        let hk = Hotkey {
            modifiers: 0x0002 | 0x0001,
            vk: 0x31,
        };
        assert_eq!(hotkey_to_string(&hk), "Ctrl+Alt+1");
        let none = Hotkey::default();
        assert_eq!(hotkey_to_string(&none), "Unassigned");
        let f5 = Hotkey {
            modifiers: 0x0008,
            vk: 0x74,
        };
        assert_eq!(hotkey_to_string(&f5), "Win+F5");
    }

    #[test]
    fn hotkey_to_string_in_uses_the_given_language() {
        use crate::i18n::Lang;
        let hk = Hotkey {
            modifiers: 0x0002 | 0x0004,
            vk: 0x2E,
        };
        assert_eq!(hotkey_to_string_in(Lang::En, &hk), "Ctrl+Shift+Delete");
        assert_eq!(hotkey_to_string_in(Lang::Es, &hk), "Ctrl+Mayús+Supr");
        assert_eq!(
            hotkey_to_string_in(Lang::Es, &Hotkey::default()),
            "Sin asignar"
        );
    }

    #[test]
    fn language_defaults_to_system_and_normalizes_tags() {
        assert_eq!(Config::default().language, "system");

        let missing: Config = serde_json::from_str(
            &serde_json::to_string(&Config::default())
                .unwrap()
                .replace("\"language\":\"system\",", ""),
        )
        .unwrap();
        assert_eq!(missing.language, "system");

        let mut cfg = Config {
            language: "ES-es".to_string(),
            ..Config::default()
        };
        cfg.normalize();
        assert_eq!(cfg.language, "es");

        cfg.language = "klingon".to_string();
        cfg.normalize();
        assert_eq!(cfg.language, "system");

        cfg.language = " System ".to_string();
        cfg.normalize();
        assert_eq!(cfg.language, "system");
    }

    #[test]
    fn config_export_and_import_roundtrip() {
        let temp_dir = std::env::temp_dir().join(format!("winspaces_test_{}", std::process::id()));
        let _ = fs::create_dir_all(&temp_dir);
        let test_file = temp_dir.join("test_export.json");

        let orig = Config {
            show_all_taskbar: true,
            space_indicator: false,
            ..Config::default()
        };
        assert!(orig.save_to_file(&test_file).is_ok());

        let imported = Config::import_from_file(&test_file).expect("Import must succeed");
        let _ = fs::remove_file(&test_file);
        let _ = fs::remove_dir(&temp_dir);

        assert_eq!(imported, orig);
    }

    /// An import must surface the failure rather than fall back to defaults —
    /// the settings window banners the message, and the user's live config
    /// stays untouched.
    #[test]
    fn config_import_rejects_an_unparseable_file() {
        let temp_dir = std::env::temp_dir().join(format!("winspaces_bad_{}", std::process::id()));
        let _ = fs::create_dir_all(&temp_dir);
        let test_file = temp_dir.join("not_a_config.json");
        let _ = fs::write(&test_file, "{ this is not json");

        let result = Config::import_from_file(&test_file);

        let _ = fs::remove_file(&test_file);
        let _ = fs::remove_dir(&temp_dir);

        assert!(result.is_err());
    }

    /// A backup written before a hotkey list grew imports as a *working*
    /// config, not one the daemon indexes past the end of.
    #[test]
    fn config_import_normalizes_a_short_hotkey_list() {
        let temp_dir = std::env::temp_dir().join(format!("winspaces_short_{}", std::process::id()));
        let _ = fs::create_dir_all(&temp_dir);
        let test_file = temp_dir.join("short.json");

        let mut short = Config::default();
        short.switch_spaces.truncate(4);
        short.move_spaces.truncate(4);
        let _ = short.save_to_file(&test_file);

        let imported = Config::import_from_file(&test_file).expect("Import must succeed");

        let _ = fs::remove_file(&test_file);
        let _ = fs::remove_dir(&temp_dir);

        assert_eq!(imported.switch_spaces.len(), MAX_SPACES);
        assert_eq!(imported.move_spaces.len(), MAX_SPACES);
    }

    /// A legacy config without a "tiling" field upgrades cleanly with default tiling settings.
    #[test]
    fn old_config_upgrades_with_tiling_defaults() {
        let json = r#"{
            "show_all_taskbar": false,
            "auto_restore_workspaces": false,
            "switch_spaces": [],
            "move_spaces": [],
            "prev": {"modifiers": 1, "vk": 37},
            "next": {"modifiers": 1, "vk": 39},
            "move_prev": {"modifiers": 13, "vk": 37},
            "move_next": {"modifiers": 13, "vk": 39}
        }"#;

        let mut cfg: Config = serde_json::from_str(json).expect("Must parse legacy config");
        cfg.normalize();

        assert!(!cfg.tiling.enabled);
        assert_eq!(cfg.tiling.inner_gap, 0);
        assert_eq!(cfg.tiling.outer_gap, 0);
        assert_eq!(cfg.tiling.ratio_step_pct, 5);
        assert_eq!(
            cfg.tiling.toggle,
            Hotkey {
                modifiers: 0x0001 | 0x0002 | 0x0004,
                vk: 0x54
            }
        );
        assert_eq!(
            cfg.tiling.focus_left,
            Hotkey {
                modifiers: 0x0001 | 0x0002 | 0x0004,
                vk: 0x25
            }
        );
        assert_eq!(
            cfg.tiling.swap_left,
            Hotkey {
                modifiers: 0x0002 | 0x0004 | 0x0008,
                vk: 0x25
            }
        );
        assert_eq!(
            cfg.tiling.ratio_shrink,
            Hotkey {
                modifiers: 0x0001 | 0x0002 | 0x0004,
                vk: 0xBD
            }
        );
        assert_eq!(
            cfg.tiling.toggle_float,
            Hotkey {
                modifiers: 0x0001 | 0x0002 | 0x0004,
                vk: 0x46
            }
        );
        assert!(cfg.tiling.float_rules.is_empty());
    }

    #[test]
    fn float_rule_as_workspace_rule_adapter_preserves_identity() {
        let rule = FloatRule {
            name: "Calculator".to_string(),
            aumid: "Microsoft.WindowsCalculator_8wekyb3d8bbwe!App".to_string(),
            exe_path: "calculatorapp.exe".to_string(),
            class_name: "ApplicationFrameWindow".to_string(),
            title_pattern: "Calculator".to_string(),
        };
        let ws = rule.as_workspace_rule();
        assert_eq!(ws.name, "Calculator");
        assert_eq!(ws.aumid, "Microsoft.WindowsCalculator_8wekyb3d8bbwe!App");
        assert_eq!(ws.exe_path, "calculatorapp.exe");
        assert_eq!(ws.class_name, "ApplicationFrameWindow");
        assert_eq!(ws.title_pattern, "Calculator");
        assert_eq!(ws.display_index, 0);
        assert_eq!(ws.space_index, 0);
    }
}
