//! Settings-window state and the config-editing actions behind the controls.
//! The settings process shares `winspaces_common::Config` with the daemon —
//! load/save/normalization/corrupt-file handling all come from that single
//! source of truth.

use super::autostart;
use super::pages::{HotkeyTarget, Page};
use crate::theme::ThemePref;
use windows_sys::Win32::UI::WindowsAndMessaging::PostMessageW;
use winspaces_common::i18n::{t, tn};
use winspaces_common::{
    log_info, tr, Config, Hotkey, Lang, Msg, PluralMsg, WM_WINSPACES_CAPTURE_WORKSPACE,
    WM_WINSPACES_RELOAD_CONFIG, WM_WINSPACES_RESTORE_WORKSPACE,
};
use winspaces_core::daemon::{find_daemon_window, is_daemon_running};

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
    pub daemon_elevated: bool,
    pub elevated_mode: bool,
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
        // Every string this process shows is read at layout/paint time, so
        // the language only needs setting before the first relayout.
        let lang = Lang::resolve(&config.language);
        winspaces_common::i18n::set_current(lang);
        log_info!(
            "Settings: UI language {:?} (setting {:?})",
            lang,
            config.language
        );
        let daemon_running = daemon_window_exists();
        let daemon_elevated = autostart::is_daemon_elevated();
        let elevated_mode = autostart::is_elevated_mode_active();
        let autostart_enabled = autostart::is_autostart_enabled();

        Self {
            config,
            page: Page::System,
            theme_pref: crate::theme::load_pref(),
            machine_name: std::env::var("COMPUTERNAME")
                .unwrap_or_else(|_| t(Msg::SettingsMachineFallback).to_string()),
            daemon_running,
            daemon_elevated,
            elevated_mode,
            autostart: autostart_enabled,
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
                log_info!("Settings: saved {} ({})", path.display(), reason);
                post_to_daemon(WM_WINSPACES_RELOAD_CONFIG);
                self.show_banner(
                    t(Msg::BannerAutosaveTitle),
                    &tr!(Msg::BannerAutosaveMessage, reason = reason),
                    true,
                );
                true
            }
            Err(e) => {
                log_info!("Settings: save to {} failed: {}", path.display(), e);
                self.show_banner(
                    t(Msg::BannerSaveFailedTitle),
                    &tr!(Msg::BannerSaveFailedMessage, path = path.display()),
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
            HotkeyTarget::TilingToggle => self.config.tiling.toggle = hk,
            HotkeyTarget::TilingFocusLeft => self.config.tiling.focus_left = hk,
            HotkeyTarget::TilingFocusRight => self.config.tiling.focus_right = hk,
            HotkeyTarget::TilingFocusUp => self.config.tiling.focus_up = hk,
            HotkeyTarget::TilingFocusDown => self.config.tiling.focus_down = hk,
            HotkeyTarget::TilingSwapLeft => self.config.tiling.swap_left = hk,
            HotkeyTarget::TilingSwapRight => self.config.tiling.swap_right = hk,
            HotkeyTarget::TilingSwapUp => self.config.tiling.swap_up = hk,
            HotkeyTarget::TilingSwapDown => self.config.tiling.swap_down = hk,
            HotkeyTarget::TilingRatioGrow => self.config.tiling.ratio_grow = hk,
            HotkeyTarget::TilingRatioShrink => self.config.tiling.ratio_shrink = hk,
            HotkeyTarget::TilingToggleFloat => self.config.tiling.toggle_float = hk,
            HotkeyTarget::TilingToggleSplit => self.config.tiling.toggle_split = hk,
        }
        self.autosave(t(Msg::ReasonHotkeyRecorded));
    }

    pub fn delete_rule(&mut self, index: usize) {
        if index < self.config.workspace_rules.len() {
            self.config.workspace_rules.remove(index);
            self.autosave(t(Msg::ReasonRuleRemoved));
        }
    }

    pub fn delete_float_rule(&mut self, index: usize) {
        if index < self.config.tiling.float_rules.len() {
            self.config.tiling.float_rules.remove(index);
            self.autosave(t(Msg::ReasonFloatRuleRemoved));
        }
    }

    pub fn reset_defaults(&mut self) {
        // Keep the language: a user who just chose Spanish and hits reset
        // should not be handed an English window.
        let language = std::mem::take(&mut self.config.language);
        self.config = Config {
            language,
            ..Config::default()
        };
        self.autosave(t(Msg::ReasonResetDefaults));
    }

    /// Write the live config to a file the user picked. Nothing else moves:
    /// this is a copy of `settings.json`, not a save, so the daemon is not
    /// notified and the config path is not touched.
    pub fn export_to_file(&mut self, path: &std::path::Path) {
        let filename = path.file_name().unwrap_or_default().to_string_lossy();
        match self.config.save_to_file(path) {
            Ok(()) => self.show_banner(
                t(Msg::BannerExportOkTitle),
                &tr!(Msg::BannerExportOkMessage, file = filename),
                true,
            ),
            Err(e) => self.show_banner(
                t(Msg::BannerExportFailedTitle),
                &tr!(Msg::BannerExportFailedMessage, file = filename, error = e),
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
                // The imported file may carry another language.
                winspaces_common::i18n::set_current(Lang::resolve(&self.config.language));
                if self.autosave(&tr!(Msg::ReasonImported, file = filename)) {
                    self.show_banner(
                        t(Msg::BannerImportOkTitle),
                        &tr!(Msg::BannerImportOkMessage, file = filename),
                        true,
                    );
                }
            }
            Err(e) => {
                self.show_banner(
                    t(Msg::BannerImportFailedTitle),
                    &tr!(Msg::BannerImportFailedMessage, file = filename, error = e),
                    false,
                );
            }
        }
    }

    pub fn toggle_elevated(&mut self) {
        let enable = !self.elevated_mode;
        let flag = if enable {
            "--enable-elevation"
        } else {
            "--disable-elevation"
        };
        match autostart::run_elevated_command(flag) {
            autostart::ElevationResult::Success => {
                self.elevated_mode = enable;
                self.refresh_daemon_status();
                if enable {
                    self.show_banner(
                        t(Msg::BannerElevatedOnTitle),
                        t(Msg::BannerElevatedOnMessage),
                        true,
                    );
                } else {
                    self.show_banner(
                        t(Msg::BannerElevatedOffTitle),
                        t(Msg::BannerElevatedOffMessage),
                        true,
                    );
                }
            }
            autostart::ElevationResult::Cancelled => {
                self.show_banner(
                    t(Msg::BannerElevationCancelledTitle),
                    t(Msg::BannerElevationCancelledMessage),
                    false,
                );
            }
            autostart::ElevationResult::Failed(err) => {
                self.show_banner(
                    t(Msg::BannerElevationFailedTitle),
                    &tr!(Msg::BannerElevationFailedMessage, error = err),
                    false,
                );
            }
        }
    }

    pub fn toggle_autostart(&mut self) {
        let enable = !self.autostart;
        if self.elevated_mode {
            let flag = if enable {
                "--enable-elevation"
            } else {
                "--disable-elevation"
            };
            match autostart::run_elevated_command(flag) {
                autostart::ElevationResult::Success => {
                    self.autostart = enable;
                    self.elevated_mode = enable;
                    self.refresh_daemon_status();
                    self.show_banner(
                        t(Msg::BannerAutostartUpdatedTitle),
                        t(if enable {
                            Msg::BannerAutostartUpdatedElevatedOn
                        } else {
                            Msg::BannerAutostartUpdatedElevatedOff
                        }),
                        true,
                    );
                }
                autostart::ElevationResult::Cancelled => {
                    self.show_banner(
                        t(Msg::BannerElevationCancelledTitle),
                        t(Msg::BannerElevationCancelledMessage),
                        false,
                    );
                }
                autostart::ElevationResult::Failed(err) => {
                    self.show_banner(
                        t(Msg::BannerAutostartFailedTitle),
                        &tr!(Msg::BannerAutostartFailedMessage, error = err),
                        false,
                    );
                }
            }
        } else {
            if autostart::set_run_key_enabled(enable) {
                self.autostart = enable;
                self.show_banner(
                    t(Msg::BannerAutostartUpdatedTitle),
                    t(if enable {
                        Msg::BannerAutostartUpdatedOn
                    } else {
                        Msg::BannerAutostartUpdatedOff
                    }),
                    true,
                );
            } else {
                self.show_banner(
                    t(Msg::BannerAutostartFailedTitle),
                    t(Msg::BannerAutostartFailedRegistry),
                    false,
                );
            }
        }
    }

    /// Re-read config after the daemon wrote captured rules (300ms contract).
    pub fn finish_capture_reload(&mut self) {
        self.config = Config::load_from_file(&Config::get_config_path());
        let count = self.config.workspace_rules.len();
        self.show_banner(
            t(Msg::BannerCaptureOkTitle),
            &tn(
                PluralMsg::BannerCaptureOkMessage,
                count as u64,
                &[("count", &count)],
            ),
            true,
        );
    }

    pub fn request_capture(&mut self) -> bool {
        post_to_daemon(WM_WINSPACES_CAPTURE_WORKSPACE)
    }

    pub fn request_restore(&mut self) {
        if post_to_daemon(WM_WINSPACES_RESTORE_WORKSPACE) {
            self.show_banner(
                t(Msg::BannerRestoreOkTitle),
                t(Msg::BannerRestoreOkMessage),
                true,
            );
        } else {
            self.show_banner(
                t(Msg::BannerDaemonMissingTitle),
                t(Msg::BannerDaemonMissingRestore),
                false,
            );
        }
        self.refresh_daemon_status();
    }

    pub fn refresh_daemon_status(&mut self) {
        self.daemon_running = daemon_window_exists();
        self.daemon_elevated = autostart::is_daemon_elevated();
        self.elevated_mode = autostart::is_elevated_mode_active();
        self.autostart = autostart::is_autostart_enabled();
    }
}

pub fn daemon_window_exists() -> bool {
    is_daemon_running()
}

pub fn post_to_daemon(msg: u32) -> bool {
    let hwnd = find_daemon_window();
    if hwnd.is_null() {
        return false;
    }
    unsafe { PostMessageW(hwnd, msg, 0, 0) != 0 }
}
