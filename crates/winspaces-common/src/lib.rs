use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

pub mod layout;

pub use layout::{
    clamp_to_work, unix_now, LayoutStore, MonitorSnapshot, RelRect, TopologySnapshot,
    WindowSnapshot,
};

/// Hard ceiling on spaces per monitor. Keeps the hotkey ID partition and the
/// digit shortcuts (Alt+1..9, Mission Control 1..9) compile-time constant
/// while the actual per-monitor count varies at runtime.
pub const MAX_DESKTOPS: usize = 9;
/// Space count a monitor starts with before the user grows or shrinks it
/// (also the serde default for snapshots captured before counts existed).
pub const DEFAULT_DESKTOPS: usize = 4;

pub const WM_WINSPACES_RELOAD_CONFIG: u32 = 0x0400 + 100; // WM_USER + 100
pub const WM_WINSPACES_CAPTURE_WORKSPACE: u32 = 0x0400 + 101; // WM_USER + 101
pub const WM_WINSPACES_RESTORE_WORKSPACE: u32 = 0x0400 + 102; // WM_USER + 102
pub const WM_WINSPACES_TOGGLE_MISSION_CONTROL: u32 = 0x0400 + 103; // WM_USER + 103
pub const WINSPACES_MSG_WINDOW_CLASS: &str = "WinSpacesMessageClass";
pub const WINSPACES_MSG_WINDOW_TITLE: &str = "WinSpacesMessageWindow";
pub const WINSPACES_DAEMON_EXE: &str = "winspaces.exe";

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
    pub desktop_index: usize,
    pub show_cmd: u32,
    pub rect: WindowRect,
    #[serde(default)]
    pub is_snapped: bool,
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
            desktop_index: 0,
            show_cmd: 1,
            rect: WindowRect::default(),
            is_snapped: false,
        }
    }
}

fn default_true() -> bool {
    true
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

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Config {
    pub show_all_taskbar: bool,
    #[serde(default)]
    pub auto_restore_workspaces: bool,
    #[serde(default = "default_true")]
    pub intercept_win_tab: bool,
    #[serde(default)]
    pub mission_control: Hotkey,
    pub switch_desktops: Vec<Hotkey>,
    pub move_desktops: Vec<Hotkey>,
    pub prev: Hotkey,
    pub next: Hotkey,
    pub move_prev: Hotkey,
    pub move_next: Hotkey,
    #[serde(default)]
    pub workspace_rules: Vec<WorkspaceRule>,
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

        let switch_desktops: Vec<Hotkey> = (0..MAX_DESKTOPS).map(default_switch_hotkey).collect();
        let move_desktops: Vec<Hotkey> = (0..MAX_DESKTOPS).map(default_move_hotkey).collect();

        Self {
            show_all_taskbar: false,
            auto_restore_workspaces: false,
            intercept_win_tab: true,
            mission_control: Hotkey {
                modifiers: MOD_CONTROL,
                vk: VK_UP,
            },
            switch_desktops,
            move_desktops,
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
            workspace_rules: Vec::new(),
        }
    }
}

/// Directory holding every file the daemon persists: portable (next to the
/// exe) when a `settings.json` already sits there, otherwise
/// `%LOCALAPPDATA%\WinSpaces`.
pub fn config_dir() -> PathBuf {
    if let Ok(mut exe_dir) = std::env::current_exe() {
        exe_dir.pop();
        if exe_dir.join("settings.json").exists() {
            return exe_dir;
        }
    }

    if let Ok(appdata) = std::env::var("LOCALAPPDATA") {
        let dir = PathBuf::from(appdata).join("WinSpaces");
        let _ = fs::create_dir_all(&dir);
        return dir;
    }

    PathBuf::from(".")
}

/// Serialize to a sibling temp file, then rename over the target. `fs::write`
/// truncates first, so a crash mid-write leaves a half-written file behind;
/// rename is atomic on NTFS, so a reader sees either the old file or the new
/// one and never a truncated one.
pub fn write_json_atomic<T: Serialize>(path: &Path, value: &T) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    let json = serde_json::to_string_pretty(value).map_err(std::io::Error::other)?;
    let tmp = path.with_extension("json.tmp");
    fs::write(&tmp, json)?;
    match fs::rename(&tmp, path) {
        Ok(()) => Ok(()),
        Err(e) => {
            let _ = fs::remove_file(&tmp);
            Err(e)
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

    /// Repair any config shape the daemon cannot safely consume. GUIs and
    /// hand-edits may produce short or oversized hotkey lists; hotkey
    /// registration indexes `switch_desktops[0..MAX_DESKTOPS]` directly.
    ///
    /// Short lists are tail-padded with the per-index *defaults* rather than
    /// unassigned entries: a settings.json written when there were only four
    /// desktops upgrades to working Alt+5..9 bindings. Explicit `vk: 0`
    /// entries inside the stored length are the user's choice and survive.
    pub fn normalize(&mut self) {
        self.switch_desktops.truncate(MAX_DESKTOPS);
        for i in self.switch_desktops.len()..MAX_DESKTOPS {
            self.switch_desktops.push(default_switch_hotkey(i));
        }
        self.move_desktops.truncate(MAX_DESKTOPS);
        for i in self.move_desktops.len()..MAX_DESKTOPS {
            self.move_desktops.push(default_move_hotkey(i));
        }
        self.sanitize_modifiers();
    }

    pub fn sanitize_modifiers(&mut self) {
        const MASK: u32 = 0x0001 | 0x0002 | 0x0004 | 0x0008;
        for hk in self
            .switch_desktops
            .iter_mut()
            .chain(self.move_desktops.iter_mut())
        {
            hk.modifiers &= MASK;
        }
        self.prev.modifiers &= MASK;
        self.next.modifiers &= MASK;
        self.move_prev.modifiers &= MASK;
        self.move_next.modifiers &= MASK;
        self.mission_control.modifiers &= MASK;
    }
}

pub fn hotkey_to_string(hk: &Hotkey) -> String {
    if hk.vk == 0 {
        return "Unassigned".to_string();
    }
    let mut parts = Vec::new();
    if (hk.modifiers & 0x0002) != 0 {
        parts.push("Ctrl");
    }
    if (hk.modifiers & 0x0001) != 0 {
        parts.push("Alt");
    }
    if (hk.modifiers & 0x0004) != 0 {
        parts.push("Shift");
    }
    if (hk.modifiers & 0x0008) != 0 {
        parts.push("Win");
    }

    let vk = hk.vk;
    let vk_str: String = if (0x41..=0x5A).contains(&vk) {
        char::from_u32(vk)
            .map(|c| c.to_string())
            .unwrap_or_default()
    } else if (0x30..=0x39).contains(&vk) {
        char::from_u32(vk)
            .map(|c| c.to_string())
            .unwrap_or_default()
    } else if (0x70..=0x87).contains(&vk) {
        format!("F{}", vk - 0x70 + 1)
    } else {
        match vk {
            0x09 => "Tab".to_string(),
            0x1B => "Esc".to_string(),
            0x20 => "Space".to_string(),
            0x0D => "Enter".to_string(),
            0x08 => "Backspace".to_string(),
            0x2E => "Delete".to_string(),
            0x24 => "Home".to_string(),
            0x23 => "End".to_string(),
            0x21 => "PageUp".to_string(),
            0x22 => "PageDown".to_string(),
            0x25 => "Left".to_string(),
            0x27 => "Right".to_string(),
            0x26 => "Up".to_string(),
            0x28 => "Down".to_string(),
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
        // A 4-entry config from before dynamic desktops upgrades to working
        // Alt+5..9 bindings, not dead unassigned slots.
        let mut cfg = Config::default();
        cfg.switch_desktops.truncate(4);
        cfg.move_desktops.truncate(1);
        cfg.normalize();
        assert_eq!(cfg.switch_desktops.len(), MAX_DESKTOPS);
        assert_eq!(cfg.move_desktops.len(), MAX_DESKTOPS);
        // Index 4 pads to Alt+5 (0x35), not Hotkey::default().
        assert_eq!(cfg.switch_desktops[4], default_switch_hotkey(4));
        assert_eq!(cfg.switch_desktops[4].vk, 0x35);
        assert_eq!(cfg.move_desktops[8], default_move_hotkey(8));
    }

    #[test]
    fn normalize_preserves_explicit_unassigned_entries() {
        // vk: 0 inside the stored length is a deliberate unbinding; only the
        // missing tail gets defaults.
        let mut cfg = Config::default();
        cfg.switch_desktops[2] = Hotkey::default();
        cfg.normalize();
        assert_eq!(cfg.switch_desktops[2], Hotkey::default());
    }

    #[test]
    fn normalize_truncates_oversized_hotkey_lists() {
        let mut cfg = Config::default();
        cfg.switch_desktops
            .extend(std::iter::repeat_n(Hotkey::default(), 10));
        cfg.normalize();
        assert_eq!(cfg.switch_desktops.len(), MAX_DESKTOPS);
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
            "switch_desktops": [],
            "move_desktops": [],
            "prev": {"modifiers": 0, "vk": 0},
            "next": {"modifiers": 0, "vk": 0},
            "move_prev": {"modifiers": 0, "vk": 0},
            "move_next": {"modifiers": 0, "vk": 0}
        }"#;
        let mut cfg: Config = serde_json::from_str(json).unwrap();
        cfg.normalize();
        assert_eq!(cfg.switch_desktops.len(), MAX_DESKTOPS);
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
}
