pub mod config;
pub mod i18n;
pub mod ipc;
pub mod layout;
pub mod logger;
pub mod paths;
pub mod spaces;

pub use config::{
    hotkey_to_string, hotkey_to_string_in, Config, FloatRule, Hotkey, TilingConfig, WindowRect,
    WorkspaceRule,
};

pub use i18n::{t, tf, tn, Lang, Msg, PluralMsg};

pub use ipc::{
    WINSPACES_DAEMON_EXE, WINSPACES_MSG_WINDOW_CLASS, WINSPACES_MSG_WINDOW_TITLE,
    WM_WINSPACES_CAPTURE_WORKSPACE, WM_WINSPACES_RELOAD_CONFIG, WM_WINSPACES_RESTORE_WORKSPACE,
    WM_WINSPACES_RETILE, WM_WINSPACES_TILING_TOGGLE, WM_WINSPACES_TOGGLE_MISSION_CONTROL,
};

pub use layout::{
    clamp_to_work, unix_now, LayoutStore, MonitorSnapshot, RelRect, TopologySnapshot,
    WindowSnapshot,
};
pub use paths::{config_dir, write_json_atomic};
pub use spaces::{DEFAULT_SPACES, MAX_SPACES};
