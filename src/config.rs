use crate::{log_error, log_info, log_warn};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

pub const NUM_DESKTOPS: usize = 4;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Hotkey {
    pub modifiers: u32,
    pub vk: u32,
}

impl Default for Hotkey {
    fn default() -> Self {
        Self { modifiers: 0, vk: 0 }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    pub show_all_taskbar: bool,
    pub switch_desktops: Vec<Hotkey>,
    pub move_desktops: Vec<Hotkey>,
    pub prev: Hotkey,
    pub next: Hotkey,
    pub move_prev: Hotkey,
    pub move_next: Hotkey,
}

impl Default for Config {
    fn default() -> Self {
        const MOD_ALT: u32 = 0x0001;
        const MOD_CONTROL: u32 = 0x0002;
        const MOD_SHIFT: u32 = 0x0004;
        const MOD_WIN: u32 = 0x0008;

        const VK_LEFT: u32 = 0x25;
        const VK_RIGHT: u32 = 0x27;

        let mut switch_desktops = Vec::with_capacity(NUM_DESKTOPS);
        let mut move_desktops = Vec::with_capacity(NUM_DESKTOPS);

        for i in 0..NUM_DESKTOPS {
            switch_desktops.push(Hotkey {
                modifiers: MOD_ALT,
                vk: 0x31 + i as u32,
            });
            move_desktops.push(Hotkey {
                modifiers: MOD_ALT | MOD_CONTROL,
                vk: 0x31 + i as u32,
            });
        }

        Self {
            show_all_taskbar: false, // WinSpaces default: false (hide windows from taskbar on inactive desktops)
            switch_desktops,
            move_desktops,
            prev: Hotkey { modifiers: MOD_ALT, vk: VK_LEFT },
            next: Hotkey { modifiers: MOD_ALT, vk: VK_RIGHT },
            move_prev: Hotkey { modifiers: MOD_ALT | MOD_SHIFT | MOD_WIN, vk: VK_LEFT },
            move_next: Hotkey { modifiers: MOD_ALT | MOD_SHIFT | MOD_WIN, vk: VK_RIGHT },
        }
    }
}

impl Config {
    pub fn get_config_path() -> PathBuf {
        if let Ok(mut exe_dir) = std::env::current_exe() {
            exe_dir.pop();
            let portable_path = exe_dir.join("settings.json");
            if portable_path.exists() {
                log_info!("Using portable config path: {:?}", portable_path);
                return portable_path;
            }
        }

        if let Ok(appdata) = std::env::var("LOCALAPPDATA") {
            let dir = PathBuf::from(appdata).join("WinSpaces");
            let _ = fs::create_dir_all(&dir);
            let path = dir.join("settings.json");
            log_info!("Using AppData config path: {:?}", path);
            return path;
        }

        PathBuf::from("settings.json")
    }

    pub fn load_from_file(path: &Path) -> Self {
        let content = match fs::read_to_string(path) {
            Ok(c) => c,
            Err(e) => {
                log_warn!("Failed to read config file at {:?}: {}. Generating defaults...", path, e);
                let default_cfg = Self::default();
                let _ = default_cfg.save_to_file(path);
                return default_cfg;
            }
        };

        match serde_json::from_str::<Config>(&content) {
            Ok(mut cfg) => {
                cfg.sanitize_modifiers();
                log_info!("Successfully loaded settings from {:?}", path);
                cfg
            }
            Err(e) => {
                log_error!("Failed to parse JSON config at {:?}: {}. Falling back to defaults.", path, e);
                let default_cfg = Self::default();
                let _ = default_cfg.save_to_file(path);
                default_cfg
            }
        }
    }

    pub fn save_to_file(&self, path: &Path) -> std::io::Result<()> {
        if let Some(parent) = path.parent() {
            let _ = fs::create_dir_all(parent);
        }
        let json = serde_json::to_string_pretty(self)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;
        fs::write(path, json)?;
        log_info!("Saved settings to {:?}", path);
        Ok(())
    }

    /// Strip any modifier bits outside the valid set (Alt/Ctrl/Shift/Win),
    /// mirroring the reference C parser's defensive masking.
    fn sanitize_modifiers(&mut self) {
        const MASK: u32 = 0x0001 | 0x0002 | 0x0004 | 0x0008;
        for hk in self.switch_desktops.iter_mut().chain(self.move_desktops.iter_mut()) {
            hk.modifiers &= MASK;
        }
        self.prev.modifiers &= MASK;
        self.next.modifiers &= MASK;
        self.move_prev.modifiers &= MASK;
        self.move_next.modifiers &= MASK;
    }
}
