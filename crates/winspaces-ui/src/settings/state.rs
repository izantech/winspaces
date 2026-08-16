//! Settings-window state and the config-editing actions behind the controls.
//! The settings process shares `winspaces_common::Config` with the daemon —
//! load/save/normalization/corrupt-file handling all come from that single
//! source of truth.

use super::autostart;
use super::pages::{HotkeyTarget, Page};
use crate::theme::ThemePref;
use windows_sys::Win32::UI::WindowsAndMessaging::{FindWindowW, PostMessageW};
use winspaces_common::{
    Config, Hotkey, WINSPACES_MSG_WINDOW_CLASS, WINSPACES_MSG_WINDOW_TITLE,
    WM_WINSPACES_CAPTURE_WORKSPACE, WM_WINSPACES_RELOAD_CONFIG, WM_WINSPACES_RESTORE_WORKSPACE,
};

use winspaces_win32::text::encode_wide;

pub struct Banner {
    pub title: String,
    pub message: String,
    pub success: bool,
}

pub struct SettingsState {
    pub config: Config,
    pub page: Page,
    pub theme_pref: ThemePref,
    pub machine_name: String,
    pub daemon_running: bool,
    pub autostart: bool,
    pub banner: Option<Banner>,
    /// Hotkey field currently recording, if any.
    pub capturing: Option<HotkeyTarget>,
    /// Whether the LL capture hook installed; false falls back to WM_KEYDOWN.
    pub capture_hook: bool,
}

impl SettingsState {
    pub fn new() -> Self {
        let config = Config::load_from_file(&Config::get_config_path());
        Self {
            config,
            page: Page::System,
            theme_pref: crate::theme::load_pref(),
            machine_name: std::env::var("COMPUTERNAME").unwrap_or_else(|_| "This PC".to_string()),
            daemon_running: daemon_window_exists(),
            autostart: autostart::is_enabled(),
            banner: None,
            capturing: None,
            capture_hook: false,
        }
    }

    pub fn show_banner(&mut self, title: &str, message: &str, success: bool) {
        self.banner = Some(Banner {
            title: title.to_string(),
            message: message.to_string(),
            success,
        });
    }

    /// Persist the config and nudge the daemon to reload it live. Returns
    /// whether the write landed, so a caller that wants to say more than the
    /// stock banner (import) does not announce success over a failed save.
    pub fn autosave(&mut self, reason: &str) -> bool {
        let path = Config::get_config_path();
        let saved = match self.config.save_to_file(&path) {
            Ok(()) => {
                post_to_daemon(WM_WINSPACES_RELOAD_CONFIG);
                self.show_banner(
                    "Auto-Saved",
                    &format!("{reason}. WinSpaces daemon reloaded live via Win32 IPC."),
                    true,
                );
                true
            }
            Err(_) => {
                self.show_banner(
                    "Save Failed",
                    &format!("Could not write settings to {}.", path.display()),
                    false,
                );
                false
            }
        };
        self.daemon_running = daemon_window_exists();
        saved
    }

    pub fn set_hotkey(&mut self, target: HotkeyTarget, hk: Hotkey) {
        match target {
            HotkeyTarget::Mission => self.config.mission_control = hk,
            HotkeyTarget::Switch(i) => {
                if i < self.config.switch_spaces.len() {
                    self.config.switch_spaces[i] = hk;
                }
            }
            HotkeyTarget::Move(i) => {
                if i < self.config.move_spaces.len() {
                    self.config.move_spaces[i] = hk;
                }
            }
            HotkeyTarget::Prev => self.config.prev = hk,
            HotkeyTarget::Next => self.config.next = hk,
            HotkeyTarget::ToggleSticky => self.config.toggle_sticky = hk,
        }
        self.autosave("Recorded new hotkey shortcut");
    }

    pub fn delete_rule(&mut self, index: usize) {
        if index < self.config.workspace_rules.len() {
            self.config.workspace_rules.remove(index);
            self.autosave("Workspace rule removed");
        }
    }

    pub fn reset_defaults(&mut self) {
        self.config = Config::default();
        self.autosave("Reset all settings to defaults");
    }

    /// Write the live config to a file the user picked. Nothing else moves:
    /// this is a copy of `settings.json`, not a save, so the daemon is not
    /// notified and the config path is not touched.
    pub fn export_to_file(&mut self, path: &std::path::Path) {
        let filename = path.file_name().unwrap_or_default().to_string_lossy();
        match self.config.save_to_file(path) {
            Ok(()) => self.show_banner(
                "Configuration Exported",
                &format!("Settings exported to {filename}."),
                true,
            ),
            Err(e) => self.show_banner(
                "Export Failed",
                &format!("Could not write {filename}: {e}"),
                false,
            ),
        }
    }

    pub fn import_from_file(&mut self, path: &std::path::Path) {
        let filename = path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned();
        match Config::import_from_file(path) {
            Ok(imported) => {
                self.config = imported;
                // Only claim the import landed if the save behind it did:
                // `autosave` banners its own failure, and overwriting that
                // with a success message would report a config the daemon
                // never received as live.
                if self.autosave(&format!("Imported configuration from {filename}")) {
                    self.show_banner(
                        "Configuration Imported",
                        &format!("Loaded settings from {filename} and updated daemon live."),
                        true,
                    );
                }
            }
            Err(e) => {
                self.show_banner(
                    "Import Failed",
                    &format!("Could not read or parse {filename}: {e}"),
                    false,
                );
            }
        }
    }

    pub fn toggle_autostart(&mut self) {
        let enable = !self.autostart;
        if autostart::set_enabled(enable) {
            self.autostart = enable;
            self.show_banner(
                "Autostart Updated",
                if enable {
                    "WinSpaces daemon will start automatically at login."
                } else {
                    "WinSpaces daemon will no longer start at login."
                },
                true,
            );
        } else {
            self.show_banner(
                "Autostart Failed",
                "Could not update the Windows startup registry entry.",
                false,
            );
        }
    }

    /// Re-read config after the daemon wrote captured rules (300ms contract).
    pub fn finish_capture_reload(&mut self) {
        self.config = Config::load_from_file(&Config::get_config_path());
        let count = self.config.workspace_rules.len();
        self.show_banner(
            "Layout Captured",
            &format!("Snapshot saved {count} window workspace rules."),
            true,
        );
    }

    pub fn request_capture(&mut self) -> bool {
        post_to_daemon(WM_WINSPACES_CAPTURE_WORKSPACE)
    }

    pub fn request_restore(&mut self) {
        if post_to_daemon(WM_WINSPACES_RESTORE_WORKSPACE) {
            self.show_banner(
                "Layout Restored",
                "Restored open windows to target display and spaces.",
                true,
            );
        } else {
            self.show_banner(
                "Daemon Not Running",
                "Start the WinSpaces daemon to restore window layouts.",
                false,
            );
        }
        self.daemon_running = daemon_window_exists();
    }

    pub fn refresh_daemon_status(&mut self) {
        self.daemon_running = daemon_window_exists();
    }
}

pub fn daemon_window_exists() -> bool {
    unsafe {
        let class_name = encode_wide(WINSPACES_MSG_WINDOW_CLASS);
        let title = encode_wide(WINSPACES_MSG_WINDOW_TITLE);
        !FindWindowW(class_name.as_ptr(), title.as_ptr()).is_null()
    }
}

pub fn post_to_daemon(msg: u32) -> bool {
    unsafe {
        let class_name = encode_wide(WINSPACES_MSG_WINDOW_CLASS);
        let title = encode_wide(WINSPACES_MSG_WINDOW_TITLE);
        let hwnd = FindWindowW(class_name.as_ptr(), title.as_ptr());
        if hwnd.is_null() {
            return false;
        }
        PostMessageW(hwnd, msg, 0, 0) != 0
    }
}
