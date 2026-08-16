//! Global daemon state: the `AppState` singleton and its accessor, panic
//! logging, menu theming, tray-icon refresh and the settings-window
//! launcher.

use std::cell::RefCell;
use windows_sys::Win32::Foundation::HWND;
use winspaces_common::{
    log_error, log_info, log_warn, Config, LayoutStore, TopologySnapshot,
    WINSPACES_MSG_WINDOW_CLASS, WINSPACES_MSG_WINDOW_TITLE,
};
use winspaces_core::spaces::SpaceManager;
use winspaces_ui::tray::TrayIcon;
use winspaces_win32::hooks::{KeyboardHook, WinEventHook};
use winspaces_win32::text::encode_wide;

thread_local! {
    pub(crate) static APP_STATE: RefCell<Option<AppState>> = const { RefCell::new(None) };
}

pub(crate) unsafe fn find_daemon_window() -> HWND {
    let class_name = encode_wide(WINSPACES_MSG_WINDOW_CLASS);
    let title = encode_wide(WINSPACES_MSG_WINDOW_TITLE);
    windows_sys::Win32::UI::WindowsAndMessaging::FindWindowW(class_name.as_ptr(), title.as_ptr())
}

/// Run `f` with mutable access to the global app state. Events arriving while
/// the state is already borrowed (re-entrant window messages) are dropped; log
/// the drop so it is visible instead of silent.
pub(crate) fn with_app_state<F: FnOnce(&mut AppState)>(f: F) {
    APP_STATE.with(|s| match s.try_borrow_mut() {
        Ok(mut state_opt) => {
            if let Some(state) = state_opt.as_mut() {
                f(state);
            }
        }
        Err(_) => {
            log_warn!("AppState busy (re-entrant event); event dropped");
        }
    });
}

/// Not `pub`: nothing outside the bin may name this type. Field order is
/// drop order — the hook RAII guards' uninstall timing depends on it, so
/// reorder fields only with that in mind.
pub(crate) struct AppState {
    pub(crate) config: Config,
    pub(crate) space_mgr: SpaceManager,
    pub(crate) tray_icon: TrayIcon,
    pub(crate) _win_event_hook: Option<WinEventHook>,
    pub(crate) _minimize_hook: Option<WinEventHook>,
    pub(crate) _keyboard_hook: Option<KeyboardHook>,

    pub(crate) message_hwnd: HWND,
    pub(crate) shell_hook_msg: u32,
    /// Persisted layouts, one per topology signature.
    pub(crate) layouts: LayoutStore,
    /// Live layout for the current topology, refreshed on a timer. This is what
    /// gets promoted into `layouts` — capturing only at `WM_DISPLAYCHANGE`
    /// would record the damage, not the layout worth restoring.
    pub(crate) shadow: Option<TopologySnapshot>,
    pub(crate) shadow_dirty: bool,
    /// Signature of the topology the last reconcile settled on.
    pub(crate) last_signature: String,
}

pub(crate) fn enable_menu_theming() {
    unsafe {
        let uxtheme_name = encode_wide("uxtheme.dll");
        let uxtheme =
            windows_sys::Win32::System::LibraryLoader::LoadLibraryW(uxtheme_name.as_ptr());
        if !uxtheme.is_null() {
            type SetPreferredAppModeFn = unsafe extern "system" fn(i32) -> i32;
            type FlushMenuThemesFn = unsafe extern "system" fn();

            let set_mode_ptr =
                windows_sys::Win32::System::LibraryLoader::GetProcAddress(uxtheme, 135 as _);
            if let Some(set_mode_raw) = set_mode_ptr {
                let set_mode: SetPreferredAppModeFn = std::mem::transmute(set_mode_raw);
                // AllowDark: classic HMENUs (the pre-Win11 tray menu fallback)
                // follow the OS apps mode instead of being forced dark.
                set_mode(1);
            }

            let flush_ptr =
                windows_sys::Win32::System::LibraryLoader::GetProcAddress(uxtheme, 133 as _);
            if let Some(flush_raw) = flush_ptr {
                let flush: FlushMenuThemesFn = std::mem::transmute(flush_raw);
                flush();
            }
        }
    }
}

/// Record panics to the log before the process dies.
///
/// The release profile builds with `panic = "abort"` and the binary is
/// `windows_subsystem = "windows"`, so a panic produces no console output, no
/// dialog, and frequently no Application Error event — the daemon simply
/// vanishes mid-session with the log ending on an unrelated line. The hook
/// still runs before the abort, which is the only chance to say what happened.
pub(crate) fn install_panic_logger() {
    std::panic::set_hook(Box::new(|info| {
        let location = info
            .location()
            .map(|l| format!("{}:{}", l.file(), l.line()))
            .unwrap_or_else(|| "unknown location".to_string());
        let msg = info
            .payload()
            .downcast_ref::<&str>()
            .map(|s| (*s).to_string())
            .or_else(|| info.payload().downcast_ref::<String>().cloned())
            .unwrap_or_else(|| "unknown payload".to_string());
        log_error!("PANIC at {}: {}", location, msg);
    }));
}

pub(crate) fn update_tray_icon() {
    with_app_state(update_state_tray_icon);
}

pub(crate) fn update_state_tray_icon(state: &mut AppState) {
    let mut text_parts = Vec::new();
    for m in &state.space_mgr.monitors {
        text_parts.push(format!("{}", m.current + 1));
    }
    let text = if text_parts.is_empty() {
        "1".to_string()
    } else {
        text_parts.join("|")
    };
    state.tray_icon.update(&text);
}

/// Open the settings window: a separate process instance of this exe, so a
/// settings crash can never take the daemon down.
pub(crate) fn launch_settings() {
    match std::env::current_exe() {
        Ok(exe) => match std::process::Command::new(&exe).arg("--settings").spawn() {
            Ok(child) => {
                log_info!("Launched settings process (PID {})", child.id());
            }
            Err(e) => {
                log_error!("Failed to spawn settings process: {}", e);
            }
        },
        Err(e) => {
            log_error!("Failed to resolve current exe for settings launch: {}", e);
        }
    }
}
