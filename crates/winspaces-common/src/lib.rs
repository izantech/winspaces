pub mod config;
pub mod hotkey_label;
pub mod i18n;
pub mod ipc;
pub mod layout;
pub mod logger;
pub mod paths;
pub mod spaces;

pub use config::{Config, FloatRule, Hotkey, WindowRect, WorkspaceRule};
pub use hotkey_label::hotkey_to_string;

pub use i18n::{t, tn, Lang, Msg, PluralMsg};

pub use ipc::{
    WINSPACES_MSG_WINDOW_CLASS, WINSPACES_MSG_WINDOW_TITLE, WM_WINSPACES_CAPTURE_WORKSPACE,
    WM_WINSPACES_RELOAD_CONFIG, WM_WINSPACES_RESTORE_WORKSPACE, WM_WINSPACES_RETILE,
    WM_WINSPACES_TILING_TOGGLE, WM_WINSPACES_TOGGLE_MISSION_CONTROL,
};

pub use layout::{
    unix_now, LayoutStore, MonitorSnapshot, RelRect, TopologySnapshot, WindowSnapshot,
};
pub use spaces::{DEFAULT_SPACES, MAX_SPACES};
