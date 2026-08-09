#![windows_subsystem = "windows"]

mod config;
mod desktop;
mod gui;
mod hooks;
mod hotkeys;
mod logger;
mod tray;

use config::Config;
use desktop::DesktopManager;
use gui::ConfigWindow;
use hooks::WinEventHook;
use hotkeys::{
    HotkeyManager, HOTKEY_ID_EXIT, HOTKEY_ID_MOVE_BASE, HOTKEY_ID_MOVE_NEXT, HOTKEY_ID_MOVE_PREV,
    HOTKEY_ID_NEXT, HOTKEY_ID_PREV, HOTKEY_ID_SPECIAL_BASE, HOTKEY_ID_SWITCH_BASE,
    HOTKEY_ID_TOGGLE,
};
use logger::Logger;
use std::cell::RefCell;
use std::ptr::null_mut;
use tray::{encode_wide, TrayIcon, WM_TRAYICON};
use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, WPARAM};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, CreatePopupMenu, CreateWindowExW, DefWindowProcW, DestroyMenu, DispatchMessageW,
    GetCursorPos, GetMessageW, MessageBoxW, PostQuitMessage, RegisterClassW, SetForegroundWindow,
    TrackPopupMenu, TranslateMessage, MB_ICONEXCLAMATION, MF_CHECKED, MF_STRING, MF_UNCHECKED, MSG,
    TPM_RIGHTBUTTON, WNDCLASSW, WS_EX_TOOLWINDOW, WS_POPUP,
};

const ID_TRAY_TOGGLE_TASKBAR: usize = 1000;
const ID_TRAY_CONFIG: usize = 1001;
const ID_TRAY_EXIT: usize = 1002;

thread_local! {
    static APP_STATE: RefCell<Option<AppState>> = const { RefCell::new(None) };
}

#[allow(dead_code)]
struct AppState {
    config: Config,
    desktop_mgr: DesktopManager,
    tray_icon: TrayIcon,
    config_window: Option<ConfigWindow>,
    _win_event_hook: Option<WinEventHook>,
    message_hwnd: HWND,
}

fn main() {
    Logger::init();
    log_info!("Starting WinSpaces initialization...");

    let config_path = Config::get_config_path();
    let config = Config::load_from_file(&config_path);

    unsafe {
        let class_name = encode_wide("WinSpacesMessageClass");
        let wc = WNDCLASSW {
            style: 0,
            lpfnWndProc: Some(wndproc),
            cbClsExtra: 0,
            cbWndExtra: 0,
            hInstance: windows_sys::Win32::System::LibraryLoader::GetModuleHandleW(null_mut()),
            hIcon: null_mut(),
            hCursor: null_mut(),
            hbrBackground: null_mut(),
            lpszMenuName: std::ptr::null(),
            lpszClassName: class_name.as_ptr(),
        };
        RegisterClassW(&wc);

        let hwnd = CreateWindowExW(
            WS_EX_TOOLWINDOW,
            class_name.as_ptr(),
            encode_wide("WinSpacesMessageWindow").as_ptr(),
            WS_POPUP,
            0,
            0,
            0,
            0,
            null_mut(),
            null_mut(),
            wc.hInstance,
            std::ptr::null(),
        );

        log_info!("Created message window handle: {:?}", hwnd);

        let mut desktop_mgr = DesktopManager::new();
        desktop_mgr.show_all_taskbar = config.show_all_taskbar;
        log_info!(
            "Initialized DesktopManager with {} monitors detected",
            desktop_mgr.monitors.len()
        );

        let tray_icon = TrayIcon::new(hwnd);
        let win_event_hook = if config.show_all_taskbar {
            WinEventHook::install(foreground_hook_proc)
        } else {
            None
        };

        let state = AppState {
            config: config.clone(),
            desktop_mgr,
            tray_icon,
            config_window: None,
            _win_event_hook: win_event_hook,
            message_hwnd: hwnd,
        };

        APP_STATE.with(|s| *s.borrow_mut() = Some(state));

        if !HotkeyManager::register_all(&config) {
            log_warn!("Hotkey registration failed at startup; opening config window.");
            APP_STATE.with(|s| {
                if let Ok(mut state_opt) = s.try_borrow_mut() {
                    if let Some(state) = state_opt.as_mut() {
                        state.desktop_mgr.handle_hotkeys = false;
                    }
                }
            });
            let _ = ConfigWindow::show(&config);
        }

        update_tray_icon();
        log_info!("Initialization complete. Entering WinMain message loop...");

        let mut msg: MSG = std::mem::zeroed();
        while GetMessageW(&mut msg, null_mut(), 0, 0) > 0 {
            if msg.message == windows_sys::Win32::UI::WindowsAndMessaging::WM_HOTKEY {
                handle_hotkey(msg.wParam as i32);
            } else {
                TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }

        log_info!("Exiting message loop. Cleaning up...");
        HotkeyManager::unregister_all();
        APP_STATE.with(|s| {
            if let Ok(mut state_opt) = s.try_borrow_mut() {
                if let Some(state) = state_opt.as_mut() {
                    state.desktop_mgr.windows_show_all();
                    state.tray_icon.remove();
                }
            }
        });
    }
}

extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    unsafe {
        match msg {
            WM_TRAYICON => {
                let event = lparam as u32;
                if event == windows_sys::Win32::UI::WindowsAndMessaging::WM_RBUTTONUP
                    || event == windows_sys::Win32::UI::WindowsAndMessaging::WM_CONTEXTMENU
                {
                    log_info!("Tray icon right-clicked");
                    show_tray_menu(hwnd);
                } else if event == windows_sys::Win32::UI::WindowsAndMessaging::WM_LBUTTONDBLCLK {
                    log_info!("Tray icon double-clicked");
                    show_config_gui();
                }
                0
            }
            windows_sys::Win32::UI::WindowsAndMessaging::WM_COMMAND => {
                let cmd = wparam & 0xffff;
                if cmd == ID_TRAY_TOGGLE_TASKBAR {
                    log_info!("Tray menu: Toggle taskbar mode");
                    APP_STATE.with(|s| {
                        if let Ok(mut state_opt) = s.try_borrow_mut() {
                            if let Some(state) = state_opt.as_mut() {
                                let new_val = !state.config.show_all_taskbar;
                                state.desktop_mgr.set_show_all_taskbar(new_val);
                                state.config.show_all_taskbar = new_val;
                                update_foreground_hook(state);
                                let _ = state.config.save_to_file(&Config::get_config_path());
                            }
                        }
                    });
                } else if cmd == ID_TRAY_CONFIG {
                    log_info!("Tray menu: Open Hotkeys config dialog");
                    show_config_gui();
                } else if cmd == ID_TRAY_EXIT {
                    log_info!("Tray menu: Exit requested");
                    PostQuitMessage(0);
                }
                0
            }
            windows_sys::Win32::UI::WindowsAndMessaging::WM_DESTROY => {
                log_info!("Window WM_DESTROY received");
                APP_STATE.with(|s| {
                    if let Ok(mut state_opt) = s.try_borrow_mut() {
                        if let Some(state) = state_opt.as_mut() {
                            state.tray_icon.remove();
                        }
                    }
                });
                PostQuitMessage(0);
                0
            }
            _ => DefWindowProcW(hwnd, msg, wparam, lparam),
        }
    }
}

pub fn step_desktop_action(delta: i32, move_window: bool) {
    APP_STATE.with(|s| {
        if let Ok(mut state_opt) = s.try_borrow_mut() {
            if let Some(state) = state_opt.as_mut() {
                if move_window {
                    state.desktop_mgr.step_move_window(delta);
                } else {
                    state.desktop_mgr.step_desktop(delta);
                }
                update_state_tray_icon(state);
            }
        }
    });
}

fn handle_hotkey(id: i32) {
    log_info!("Received WM_HOTKEY message for ID {}", id);
    if id == HOTKEY_ID_EXIT {
        log_info!("Hotkey exit requested");
        unsafe {
            PostQuitMessage(0);
        }
        return;
    }
    APP_STATE.with(|s| {
        if let Ok(mut state_opt) = s.try_borrow_mut() {
            if let Some(state) = state_opt.as_mut() {
                if (HOTKEY_ID_SWITCH_BASE..HOTKEY_ID_MOVE_BASE).contains(&id) {
                    let desk = (id - HOTKEY_ID_SWITCH_BASE) as usize;
                    state.desktop_mgr.go_to_desk(desk);
                    update_state_tray_icon(state);
                } else if (HOTKEY_ID_MOVE_BASE..HOTKEY_ID_SPECIAL_BASE).contains(&id) {
                    let desk = (id - HOTKEY_ID_MOVE_BASE) as usize;
                    state.desktop_mgr.move_to_desk(desk);
                    update_state_tray_icon(state);
                } else if id == HOTKEY_ID_PREV {
                    state.desktop_mgr.step_desktop(-1);
                    update_state_tray_icon(state);
                } else if id == HOTKEY_ID_NEXT {
                    state.desktop_mgr.step_desktop(1);
                    update_state_tray_icon(state);
                } else if id == HOTKEY_ID_MOVE_PREV {
                    state.desktop_mgr.step_move_window(-1);
                    update_state_tray_icon(state);
                } else if id == HOTKEY_ID_MOVE_NEXT {
                    state.desktop_mgr.step_move_window(1);
                    update_state_tray_icon(state);
                } else if id == HOTKEY_ID_TOGGLE {
                    toggle_hotkeys(state);
                }
            }
        }
    });
}

/// Commit a new config: update AppState, persist, and re-register hotkeys.
/// On registration failure the previous config is restored and `false` is
/// returned so the caller can resync its pending copy.
pub fn apply_config(new_config: Config) -> bool {
    let mut success = true;
    APP_STATE.with(|s| {
        if let Ok(mut state_opt) = s.try_borrow_mut() {
            if let Some(state) = state_opt.as_mut() {
                let old = state.config.clone();
                state.config = new_config;
                let _ = state.config.save_to_file(&Config::get_config_path());
                if state.desktop_mgr.handle_hotkeys {
                    HotkeyManager::unregister_all();
                    if !HotkeyManager::register_all(&state.config) {
                        unsafe {
                            MessageBoxW(
                                null_mut(),
                                encode_wide(
                                    "Hotkey registration failed. Please choose different hotkeys.",
                                )
                                .as_ptr(),
                                encode_wide("WinSpaces").as_ptr(),
                                MB_ICONEXCLAMATION,
                            );
                        }
                        state.config = old;
                        let _ = state.config.save_to_file(&Config::get_config_path());
                        HotkeyManager::register_all(&state.config);
                        success = false;
                    }
                }
            }
        }
    });
    success
}

pub fn get_config() -> Config {
    APP_STATE.with(|s| {
        s.try_borrow()
            .ok()
            .and_then(|st| st.as_ref().map(|s| s.config.clone()))
            .unwrap_or_default()
    })
}

fn toggle_hotkeys(state: &mut AppState) {
    state.desktop_mgr.handle_hotkeys = !state.desktop_mgr.handle_hotkeys;
    if state.desktop_mgr.handle_hotkeys {
        if !HotkeyManager::register_all(&state.config) {
            unsafe {
                MessageBoxW(
                    null_mut(),
                    encode_wide("Hotkey registration failed. Please adjust the hotkeys.").as_ptr(),
                    encode_wide("WinSpaces").as_ptr(),
                    MB_ICONEXCLAMATION,
                );
            }
            let _ = ConfigWindow::show(&state.config);
            state.desktop_mgr.handle_hotkeys = false;
        }
    } else {
        HotkeyManager::unregister_all();
    }
}

fn update_state_tray_icon(state: &mut AppState) {
    let n = state.desktop_mgr.monitors.len();
    let label: String = if n == 0 {
        "1".to_string()
    } else {
        let mut s = String::new();
        for (i, m) in state.desktop_mgr.monitors.iter().enumerate() {
            s.push(char::from_digit((m.current + 1) as u32, 10).unwrap_or('1'));
            if i + 1 < n {
                s.push('/');
            }
        }
        s
    };
    state.tray_icon.update(&label);
}

fn update_tray_icon() {
    APP_STATE.with(|s| {
        if let Ok(mut state_opt) = s.try_borrow_mut() {
            if let Some(state) = state_opt.as_mut() {
                update_state_tray_icon(state);
            }
        }
    });
}

fn show_config_gui() {
    APP_STATE.with(|s| {
        if let Ok(state_opt) = s.try_borrow() {
            if let Some(state) = state_opt.as_ref() {
                let _ = ConfigWindow::show(&state.config);
            }
        }
    });
}

fn show_tray_menu(hwnd: HWND) {
    unsafe {
        let hmenu = CreatePopupMenu();
        let mut pt = POINT { x: 0, y: 0 };
        GetCursorPos(&mut pt);

        let show_tb = APP_STATE.with(|s| {
            s.try_borrow()
                .ok()
                .and_then(|st| st.as_ref().map(|s| s.config.show_all_taskbar))
                .unwrap_or(true)
        });

        let flags_tb = if show_tb { MF_CHECKED } else { MF_UNCHECKED };
        AppendMenuW(
            hmenu,
            flags_tb,
            ID_TRAY_TOGGLE_TASKBAR,
            encode_wide("Show all windows on taskbar").as_ptr(),
        );
        AppendMenuW(
            hmenu,
            MF_STRING,
            ID_TRAY_CONFIG,
            encode_wide("Configure Hotkeys...").as_ptr(),
        );
        AppendMenuW(
            hmenu,
            MF_STRING,
            ID_TRAY_EXIT,
            encode_wide("Exit WinSpaces").as_ptr(),
        );

        SetForegroundWindow(hwnd);
        TrackPopupMenu(
            hmenu,
            TPM_RIGHTBUTTON,
            pt.x,
            pt.y,
            0,
            hwnd,
            std::ptr::null(),
        );
        DestroyMenu(hmenu);
    }
}

unsafe extern "system" fn foreground_hook_proc(
    _: windows_sys::Win32::UI::Accessibility::HWINEVENTHOOK,
    _: u32,
    hwnd: HWND,
    id_object: i32,
    id_child: i32,
    _: u32,
    _: u32,
) {
    if id_object != 0 || id_child != 0 {
        return;
    }
    APP_STATE.with(|s| {
        if let Ok(mut state_opt) = s.try_borrow_mut() {
            if let Some(state) = state_opt.as_mut() {
                let mgr = &mut state.desktop_mgr;
                if !mgr.show_all_taskbar || mgr.suppress_foreground {
                    return;
                }
                if !desktop::is_valid_window(hwnd) {
                    return;
                }
                let (mon_idx, desk_idx) = match mgr.find_window(hwnd) {
                    Some(loc) => loc,
                    None => return,
                };
                let now = windows_sys::Win32::System::SystemInformation::GetTickCount();
                let mon = &mut mgr.monitors[mon_idx];
                if mon.suppress_foreground_until != 0 {
                    if (now as i32).wrapping_sub(mon.suppress_foreground_until as i32) < 0 {
                        return;
                    }
                    mon.suppress_foreground_until = 0;
                }
                if mon.current == desk_idx {
                    return;
                }
                mgr.suppress_foreground = true;
                mgr.switch_desktop(mon_idx, desk_idx, Some(hwnd));
                mgr.suppress_foreground = false;
            }
        }
    });
}

fn update_foreground_hook(state: &mut AppState) {
    if state.desktop_mgr.show_all_taskbar && state._win_event_hook.is_none() {
        state._win_event_hook = WinEventHook::install(foreground_hook_proc);
    } else if !state.desktop_mgr.show_all_taskbar && state._win_event_hook.is_some() {
        state._win_event_hook = None;
    }
}
