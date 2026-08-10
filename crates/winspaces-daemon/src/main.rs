#![windows_subsystem = "windows"]

mod desktop;
mod hooks;
mod hotkeys;
mod logger;
mod mission_control;
mod tray;
mod workspaces;

use desktop::DesktopManager;
use hooks::{KeyboardHook, WinEventHook};
use hotkeys::{
    HotkeyManager, HOTKEY_ID_EXIT, HOTKEY_ID_MISSION_CONTROL, HOTKEY_ID_MOVE_BASE,
    HOTKEY_ID_MOVE_NEXT, HOTKEY_ID_MOVE_PREV, HOTKEY_ID_NEXT, HOTKEY_ID_PREV,
    HOTKEY_ID_SPECIAL_BASE, HOTKEY_ID_SWITCH_BASE, HOTKEY_ID_TOGGLE,
};
use logger::Logger;
use std::cell::RefCell;
use std::ptr::null_mut;
use tray::{encode_wide, TrayIcon, WM_TRAYICON};
use windows_sys::Win32::Foundation::{HMODULE, HWND, LPARAM, LRESULT, POINT, WPARAM};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, CreatePopupMenu, CreateWindowExW, DefWindowProcW, DestroyMenu, DispatchMessageW,
    GetAncestor, GetCursorPos, GetMessageW, PostQuitMessage, RegisterClassW,
    RegisterShellHookWindow, RegisterWindowMessageW, SetForegroundWindow, TrackPopupMenu,
    TranslateMessage, GA_ROOTOWNER, HSHELL_WINDOWACTIVATED, HSHELL_WINDOWCREATED, MF_CHECKED,
    MF_DISABLED, MF_POPUP, MF_SEPARATOR, MF_STRING, MF_UNCHECKED, MSG, TPM_RIGHTBUTTON, WNDCLASSW,
    WS_EX_TOOLWINDOW, WS_POPUP,
};
use winspaces_common::{
    Config, WINSPACES_MSG_WINDOW_CLASS, WINSPACES_MSG_WINDOW_TITLE, WM_WINSPACES_CAPTURE_WORKSPACE,
    WM_WINSPACES_RELOAD_CONFIG, WM_WINSPACES_RESTORE_WORKSPACE,
    WM_WINSPACES_TOGGLE_MISSION_CONTROL,
};

const HSHELL_RUDEAPPACTIVATED: u32 = HSHELL_WINDOWACTIVATED | 0x8000;

const ID_TRAY_MISSION_CONTROL: usize = 999;
const ID_TRAY_TOGGLE_TASKBAR: usize = 1000;
const ID_TRAY_CONFIG: usize = 1001;
const ID_TRAY_EXIT: usize = 1002;
const ID_TRAY_RELOAD: usize = 1003;
const ID_TRAY_CAPTURE_WS: usize = 1004;
const ID_TRAY_RESTORE_WS: usize = 1005;
const ID_TRAY_SWITCH_BASE: usize = 2000;

thread_local! {
    static APP_STATE: RefCell<Option<AppState>> = const { RefCell::new(None) };
}

pub fn get_app_instance() -> HMODULE {
    unsafe { windows_sys::Win32::System::LibraryLoader::GetModuleHandleW(null_mut()) }
}

unsafe fn find_daemon_window() -> HWND {
    let class_name = encode_wide(WINSPACES_MSG_WINDOW_CLASS);
    let title = encode_wide(WINSPACES_MSG_WINDOW_TITLE);
    windows_sys::Win32::UI::WindowsAndMessaging::FindWindowW(class_name.as_ptr(), title.as_ptr())
}

/// Run `f` with mutable access to the global app state. Events arriving while
/// the state is already borrowed (re-entrant window messages) are dropped; log
/// the drop so it is visible instead of silent.
fn with_app_state<F: FnOnce(&mut AppState)>(f: F) {
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

struct AppState {
    config: Config,
    desktop_mgr: DesktopManager,
    tray_icon: TrayIcon,
    _win_event_hook: Option<WinEventHook>,
    _keyboard_hook: Option<KeyboardHook>,
    message_hwnd: HWND,
    shell_hook_msg: u32,
}

fn enable_dark_mode_menu() {
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
                set_mode(2); // ForceDark
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

unsafe extern "system" fn low_level_keyboard_proc(
    code: i32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    if code >= 0
        && (wparam == windows_sys::Win32::UI::WindowsAndMessaging::WM_KEYDOWN as usize
            || wparam == windows_sys::Win32::UI::WindowsAndMessaging::WM_SYSKEYDOWN as usize)
    {
        let kbd = &*(lparam as *const windows_sys::Win32::UI::WindowsAndMessaging::KBDLLHOOKSTRUCT);
        if kbd.vkCode == windows_sys::Win32::UI::Input::KeyboardAndMouse::VK_TAB as u32 {
            let win_down = (windows_sys::Win32::UI::Input::KeyboardAndMouse::GetAsyncKeyState(
                windows_sys::Win32::UI::Input::KeyboardAndMouse::VK_LWIN as i32,
            ) as u16
                & 0x8000
                != 0)
                || (windows_sys::Win32::UI::Input::KeyboardAndMouse::GetAsyncKeyState(
                    windows_sys::Win32::UI::Input::KeyboardAndMouse::VK_RWIN as i32,
                ) as u16
                    & 0x8000
                    != 0);
            if win_down {
                // Do the minimum inside the LL hook: exceeding the system's
                // low-level-hook timeout gets the hook silently uninstalled.
                // Post the toggle to the message loop instead.
                let mut handled = false;
                APP_STATE.with(|s| {
                    if let Ok(state_opt) = s.try_borrow() {
                        if let Some(state) = state_opt.as_ref() {
                            if state.config.intercept_win_tab {
                                windows_sys::Win32::UI::WindowsAndMessaging::PostMessageW(
                                    state.message_hwnd,
                                    WM_WINSPACES_TOGGLE_MISSION_CONTROL,
                                    0,
                                    0,
                                );
                                handled = true;
                            }
                        }
                    }
                });
                if handled {
                    // Inject a no-op key so the swallowed Tab still counts as
                    // "a key was pressed while Win was down" — otherwise the
                    // Start menu opens when the Win key is released.
                    windows_sys::Win32::UI::Input::KeyboardAndMouse::keybd_event(0xFF, 0, 0, 0);
                    windows_sys::Win32::UI::Input::KeyboardAndMouse::keybd_event(
                        0xFF,
                        0,
                        windows_sys::Win32::UI::Input::KeyboardAndMouse::KEYEVENTF_KEYUP,
                        0,
                    );
                    return 1;
                }
            }
        }
    }
    windows_sys::Win32::UI::WindowsAndMessaging::CallNextHookEx(
        std::ptr::null_mut(),
        code,
        wparam,
        lparam,
    )
}

fn main() {
    unsafe {
        windows_sys::Win32::UI::HiDpi::SetProcessDpiAwarenessContext(
            windows_sys::Win32::UI::HiDpi::DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
        );
        windows_sys::Win32::System::Console::AttachConsole(
            windows_sys::Win32::System::Console::ATTACH_PARENT_PROCESS,
        );
    }
    Logger::init();
    log_info!("Starting WinSpaces daemon (v0.1.0)...");
    enable_dark_mode_menu();
    unsafe {
        windows_sys::Win32::System::Com::CoInitializeEx(
            null_mut(),
            windows_sys::Win32::System::Com::COINIT_APARTMENTTHREADED as _,
        );
    }
    mission_control::init_mission_control();

    let config_path = Config::get_config_path();
    let config = Config::load_from_file(&config_path);

    let args: Vec<String> = std::env::args().collect();
    if args.len() > 1 && (args[1] == "--exit" || args[1] == "--kill") {
        // Message the running daemon if there is one; never boot a new daemon
        // from a control command.
        unsafe {
            let hwnd = find_daemon_window();
            if !hwnd.is_null() {
                windows_sys::Win32::UI::WindowsAndMessaging::PostMessageW(
                    hwnd,
                    windows_sys::Win32::UI::WindowsAndMessaging::WM_COMMAND,
                    ID_TRAY_EXIT as _,
                    0,
                );
            } else {
                log_warn!("--exit requested but no running daemon was found");
            }
        }
        return;
    }
    if args.len() > 1 && (args[1] == "--mission-control" || args[1] == "-m") {
        unsafe {
            let hwnd = find_daemon_window();
            if !hwnd.is_null() {
                windows_sys::Win32::UI::WindowsAndMessaging::PostMessageW(
                    hwnd,
                    WM_WINSPACES_TOGGLE_MISSION_CONTROL,
                    0,
                    0,
                );
            } else {
                log_warn!("--mission-control requested but no running daemon was found");
            }
        }
        return;
    }
    if args.len() > 1 && args[1] == "--dump" {
        let out_file = if args.len() > 2 {
            &args[2]
        } else {
            "window_dump.txt"
        };
        let mgr = DesktopManager::new();
        unsafe {
            workspaces::dump_all_window_metrics(&mgr, out_file);
        }
        return;
    }
    log_info!(
        "Loaded config with {} switch hotkeys, {} move hotkeys, {} workspace rules",
        config.switch_desktops.len(),
        config.move_desktops.len(),
        config.workspace_rules.len()
    );

    unsafe {
        let instance = windows_sys::Win32::System::LibraryLoader::GetModuleHandleW(null_mut());
        let class_name = encode_wide(WINSPACES_MSG_WINDOW_CLASS);
        let window_title = encode_wide(WINSPACES_MSG_WINDOW_TITLE);

        let wc = WNDCLASSW {
            style: 0,
            lpfnWndProc: Some(wndproc),
            cbClsExtra: 0,
            cbWndExtra: 0,
            hInstance: instance,
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
            window_title.as_ptr(),
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

        // The daemon may run elevated while the GUI/CLI run at medium
        // integrity; UIPI silently drops their messages unless allowed here.
        {
            use windows_sys::Win32::UI::WindowsAndMessaging::{
                ChangeWindowMessageFilterEx, MSGFLT_ALLOW, WM_COMMAND,
            };
            for msg in [
                WM_WINSPACES_RELOAD_CONFIG,
                WM_WINSPACES_CAPTURE_WORKSPACE,
                WM_WINSPACES_RESTORE_WORKSPACE,
                WM_WINSPACES_TOGGLE_MISSION_CONTROL,
                WM_COMMAND,
            ] {
                ChangeWindowMessageFilterEx(hwnd, msg, MSGFLT_ALLOW, std::ptr::null_mut());
            }
        }

        // Register Shell Hook for auto-placing launched windows
        RegisterShellHookWindow(hwnd);
        let shell_hook_name = encode_wide("SHELLHOOK");
        let shell_hook_msg = RegisterWindowMessageW(shell_hook_name.as_ptr());
        log_info!("Registered ShellHook message ID: {}", shell_hook_msg);

        let mut desktop_mgr = DesktopManager::new();
        desktop_mgr.show_all_taskbar = config.show_all_taskbar;
        log_info!(
            "Initialized DesktopManager with {} monitors detected",
            desktop_mgr.monitors.len()
        );

        let tray_icon = TrayIcon::new(hwnd);
        let win_event_hook = WinEventHook::install(foreground_hook_proc);

        let keyboard_hook = KeyboardHook::install(Some(low_level_keyboard_proc));

        let mut state = AppState {
            config: config.clone(),
            desktop_mgr,
            tray_icon,
            _win_event_hook: win_event_hook,
            _keyboard_hook: keyboard_hook,
            message_hwnd: hwnd,
            shell_hook_msg,
        };

        if config.auto_restore_workspaces && !config.workspace_rules.is_empty() {
            log_info!("Auto-restoring workspace window rules on startup...");
            restore_workspace_rules(&mut state);
        }

        APP_STATE.with(|s| *s.borrow_mut() = Some(state));

        if !HotkeyManager::register_all(&config) {
            log_warn!("Hotkey registration failed at startup; launching GUI configurator.");
            with_app_state(|state| {
                state.desktop_mgr.handle_hotkeys = false;
            });
            launch_gui();
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
        with_app_state(|state| {
            state.desktop_mgr.windows_show_all();
            state.tray_icon.remove();
        });
    }
}

fn restore_workspace_rules(state: &mut AppState) {
    if state.config.workspace_rules.is_empty() {
        return;
    }
    unsafe {
        windows_sys::Win32::UI::WindowsAndMessaging::EnumWindows(
            Some(restore_enum_proc),
            state as *mut _ as isize,
        );
    }
    for mon_idx in 0..state.desktop_mgr.monitors.len() {
        let cur = state.desktop_mgr.monitors[mon_idx].current;
        state.desktop_mgr.switch_desktop(mon_idx, cur, None);
    }
}

unsafe extern "system" fn restore_enum_proc(hwnd: HWND, lparam: isize) -> i32 {
    let state = &mut *(lparam as *mut AppState);
    if let Some(rule) = workspaces::match_rule_for_window(hwnd, &state.config.workspace_rules) {
        log_info!(
            "Restoring window {:?} under rule '{}' -> Display {}, Space {}",
            hwnd,
            rule.name,
            rule.display_index + 1,
            rule.desktop_index + 1
        );
        workspaces::apply_rule_to_window(hwnd, &rule);
        state
            .desktop_mgr
            .track_window(hwnd, rule.display_index, rule.desktop_index);
    }
    1
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
                } else if event == windows_sys::Win32::UI::WindowsAndMessaging::WM_LBUTTONUP {
                    // Every left click toggles instantly. Double-click has no
                    // separate meaning (Settings lives in the context menu), so
                    // no need to defer past the double-click interval.
                    log_info!("Tray icon left-clicked: Toggling Mission Control");
                    with_app_state(|state| {
                        mission_control::toggle_mission_control(state);
                    });
                }
                0
            }
            windows_sys::Win32::UI::WindowsAndMessaging::WM_COMMAND => {
                let cmd = wparam & 0xffff;
                if cmd == ID_TRAY_MISSION_CONTROL {
                    log_info!("Tray menu: Mission Control requested");
                    with_app_state(|state| {
                        mission_control::toggle_mission_control(state);
                    });
                } else if cmd == ID_TRAY_TOGGLE_TASKBAR {
                    log_info!("Tray menu: Toggle taskbar mode");
                    with_app_state(|state| {
                        let new_val = !state.config.show_all_taskbar;
                        state.desktop_mgr.set_show_all_taskbar(new_val);
                        state.config.show_all_taskbar = new_val;
                        update_foreground_hook(state);
                        let _ = state.config.save_to_file(&Config::get_config_path());
                    });
                } else if cmd == ID_TRAY_CONFIG {
                    log_info!("Tray menu: Open Hotkeys config GUI requested");
                    launch_gui();
                } else if cmd == ID_TRAY_RELOAD {
                    log_info!("Tray menu: Reload requested");
                    with_app_state(|state| {
                        let path = Config::get_config_path();
                        let new_config = Config::load_from_file(&path);
                        state.config = new_config.clone();
                        state
                            .desktop_mgr
                            .set_show_all_taskbar(new_config.show_all_taskbar);
                        update_foreground_hook(state);
                        HotkeyManager::unregister_all();
                        let _ = HotkeyManager::register_all(&state.config);
                        update_state_tray_icon(state);
                    });
                } else if cmd == ID_TRAY_CAPTURE_WS {
                    log_info!("Tray menu: Capture Workspace requested");
                    with_app_state(|state| {
                        let rules = workspaces::capture_active_workspace(&state.desktop_mgr);
                        log_info!("Captured {} workspace rules", rules.len());
                        state.config.workspace_rules = rules;
                        let path = Config::get_config_path();
                        let _ = state.config.save_to_file(&path);
                    });
                } else if cmd == ID_TRAY_RESTORE_WS {
                    log_info!("Tray menu: Restore Workspace requested");
                    with_app_state(|state| {
                        restore_workspace_rules(state);
                    });
                } else if cmd == ID_TRAY_EXIT {
                    log_info!("Tray menu: Exit requested");
                    PostQuitMessage(0);
                } else if cmd >= ID_TRAY_SWITCH_BASE {
                    let offset = cmd - ID_TRAY_SWITCH_BASE;
                    let mon_idx = offset / 100;
                    let desk_idx = offset % 100;
                    log_info!(
                        "Tray menu: Switch Monitor {} to Desktop {}",
                        mon_idx + 1,
                        desk_idx + 1
                    );
                    with_app_state(|state| {
                        state.desktop_mgr.switch_desktop(mon_idx, desk_idx, None);
                        update_state_tray_icon(state);
                    });
                }
                0
            }
            WM_WINSPACES_RELOAD_CONFIG => {
                log_info!("Received configuration reload IPC message from GUI");
                with_app_state(|state| {
                    let path = Config::get_config_path();
                    let new_config = Config::load_from_file(&path);
                    state.config = new_config.clone();
                    state
                        .desktop_mgr
                        .set_show_all_taskbar(new_config.show_all_taskbar);
                    update_foreground_hook(state);
                    HotkeyManager::unregister_all();
                    if !HotkeyManager::register_all(&state.config) {
                        log_warn!("Hotkey registration failed after IPC config reload.");
                    }
                    update_state_tray_icon(state);
                });
                0
            }
            WM_WINSPACES_CAPTURE_WORKSPACE => {
                log_info!("Received capture workspace IPC message from GUI");
                with_app_state(|state| {
                    let rules = workspaces::capture_active_workspace(&state.desktop_mgr);
                    log_info!("Captured {} workspace rules from layout", rules.len());
                    state.config.workspace_rules = rules;
                    let path = Config::get_config_path();
                    let _ = state.config.save_to_file(&path);
                });
                0
            }
            WM_WINSPACES_RESTORE_WORKSPACE => {
                log_info!("Received restore workspace IPC message from GUI");
                with_app_state(|state| {
                    restore_workspace_rules(state);
                });
                0
            }
            WM_WINSPACES_TOGGLE_MISSION_CONTROL => {
                log_info!("Received toggle mission control IPC message");
                with_app_state(|state| {
                    mission_control::toggle_mission_control(state);
                });
                0
            }
            windows_sys::Win32::UI::WindowsAndMessaging::WM_DISPLAYCHANGE => {
                log_info!("Display topology changed; remapping monitors");
                with_app_state(|state| {
                    state.desktop_mgr.handle_display_change();
                    update_state_tray_icon(state);
                });
                0
            }
            windows_sys::Win32::UI::WindowsAndMessaging::WM_DESTROY => {
                log_info!("Window WM_DESTROY received");
                with_app_state(|state| {
                    state.tray_icon.remove();
                });
                PostQuitMessage(0);
                0
            }
            _ => {
                let shell_hook_id = APP_STATE.with(|s| {
                    s.try_borrow()
                        .ok()
                        .and_then(|st| st.as_ref().map(|s| s.shell_hook_msg))
                        .unwrap_or(0)
                });
                if shell_hook_id != 0 && msg == shell_hook_id {
                    let event = wparam as u32;
                    let target_hwnd = lparam as HWND;
                    if event == HSHELL_WINDOWCREATED {
                        with_app_state(|state| {
                            if state.config.auto_restore_workspaces {
                                if let Some(rule) = workspaces::match_rule_for_window(
                                    target_hwnd,
                                    &state.config.workspace_rules,
                                ) {
                                    log_info!(
                                        "ShellHook auto-placing window {:?} under rule '{}' -> Display {}, Space {}",
                                        target_hwnd,
                                        rule.name,
                                        rule.display_index + 1,
                                        rule.desktop_index + 1
                                    );
                                    workspaces::apply_rule_to_window(target_hwnd, &rule);
                                    state.desktop_mgr.track_window(
                                        target_hwnd,
                                        rule.display_index,
                                        rule.desktop_index,
                                    );
                                    state.desktop_mgr.switch_desktop(
                                        rule.display_index,
                                        rule.desktop_index,
                                        Some(target_hwnd),
                                    );
                                    return;
                                }
                            }
                            state.desktop_mgr.scan_untracked_windows();
                        });
                    } else if event == HSHELL_WINDOWACTIVATED
                        || event == HSHELL_RUDEAPPACTIVATED
                        || (event & 0x7FFF) == HSHELL_WINDOWACTIVATED
                    {
                        with_app_state(|state| {
                            handle_window_activated(target_hwnd, state);
                        });
                    }
                    0
                } else {
                    DefWindowProcW(hwnd, msg, wparam, lparam)
                }
            }
        }
    }
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
    with_app_state(|state| {
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
        } else if id == HOTKEY_ID_MISSION_CONTROL {
            mission_control::toggle_mission_control(state);
        }
    });
}

fn toggle_hotkeys(state: &mut AppState) {
    state.desktop_mgr.handle_hotkeys = !state.desktop_mgr.handle_hotkeys;
    if state.desktop_mgr.handle_hotkeys {
        if !HotkeyManager::register_all(&state.config) {
            log_warn!("Hotkey re-registration failed upon toggle; opening GUI configurator.");
            launch_gui();
            state.desktop_mgr.handle_hotkeys = false;
        }
    } else {
        HotkeyManager::unregister_all();
    }
}

fn update_tray_icon() {
    with_app_state(update_state_tray_icon);
}

fn update_state_tray_icon(state: &mut AppState) {
    let mut text_parts = Vec::new();
    for m in &state.desktop_mgr.monitors {
        text_parts.push(format!("{}", m.current + 1));
    }
    let text = if text_parts.is_empty() {
        "1".to_string()
    } else {
        text_parts.join("|")
    };
    state.tray_icon.update(&text);
}

fn launch_gui() {
    log_info!("Attempting to launch GUI configurator (WinSpaces.Gui.exe)...");

    if let Ok(exe_path) = std::env::current_exe() {
        if let Some(parent) = exe_path.parent() {
            let gui_exe = parent.join(winspaces_common::WINSPACES_GUI_EXE);
            if gui_exe.exists() {
                log_info!("Found GUI executable at {:?}", gui_exe);
                let mut cmd = std::process::Command::new(&gui_exe);
                match cmd.spawn() {
                    Ok(child) => {
                        log_info!("Successfully launched GUI process (PID {})", child.id());
                        return;
                    }
                    Err(e) => {
                        log_error!("Failed to spawn GUI process: {}", e);
                    }
                }
            }
        }
    }

    log_warn!("Could not find GUI configurator executable.");
}

fn show_tray_menu(hwnd: HWND) {
    unsafe {
        let hmenu = CreatePopupMenu();
        let mut pt = POINT { x: 0, y: 0 };
        GetCursorPos(&mut pt);

        let (show_tb, monitors_info) = APP_STATE.with(|s| {
            s.try_borrow()
                .ok()
                .and_then(|st| {
                    st.as_ref().map(|state| {
                        let show_tb = state.config.show_all_taskbar;
                        let mons: Vec<(usize, usize)> = state
                            .desktop_mgr
                            .monitors
                            .iter()
                            .enumerate()
                            .map(|(idx, m)| (idx, m.current))
                            .collect();
                        (show_tb, mons)
                    })
                })
                .unwrap_or((true, vec![(0, 0)]))
        });

        // 1. Header item showing overall status
        let mut status_str = String::from("WinSpaces");
        for (i, (_, curr)) in monitors_info.iter().enumerate() {
            status_str.push_str(&format!("  •  Disp {}: Space {}", i + 1, curr + 1));
        }

        AppendMenuW(
            hmenu,
            MF_DISABLED | MF_STRING,
            0,
            encode_wide(&status_str).as_ptr(),
        );
        AppendMenuW(hmenu, MF_SEPARATOR, 0, std::ptr::null());

        // 2. Mission Control
        AppendMenuW(
            hmenu,
            MF_STRING,
            ID_TRAY_MISSION_CONTROL,
            encode_wide("🪟 Mission Control  (Win+Tab)").as_ptr(),
        );
        AppendMenuW(hmenu, MF_SEPARATOR, 0, std::ptr::null());

        // 3. Submenus for switching space per display
        for (mon_idx, curr_space) in &monitors_info {
            let hsub = CreatePopupMenu();
            for desk_idx in 0..4 {
                let cmd_id = ID_TRAY_SWITCH_BASE + (mon_idx * 100) + desk_idx;
                let flags = if *curr_space == desk_idx {
                    MF_CHECKED | MF_STRING
                } else {
                    MF_UNCHECKED | MF_STRING
                };
                let label = format!("Space {}  (Alt+{})", desk_idx + 1, desk_idx + 1);
                AppendMenuW(hsub, flags, cmd_id, encode_wide(&label).as_ptr());
            }

            let sub_label = format!("Display {} (Space {})", mon_idx + 1, curr_space + 1);
            AppendMenuW(
                hmenu,
                MF_POPUP,
                hsub as usize,
                encode_wide(&sub_label).as_ptr(),
            );
        }

        AppendMenuW(hmenu, MF_SEPARATOR, 0, std::ptr::null());

        // 3. Workspaces Layout Actions
        AppendMenuW(
            hmenu,
            MF_STRING,
            ID_TRAY_CAPTURE_WS,
            encode_wide("📸 Capture Current Layout as Workspace").as_ptr(),
        );
        AppendMenuW(
            hmenu,
            MF_STRING,
            ID_TRAY_RESTORE_WS,
            encode_wide("📐 Restore Workspace Window Layout").as_ptr(),
        );

        AppendMenuW(hmenu, MF_SEPARATOR, 0, std::ptr::null());

        // 4. Toggle Taskbar & Settings
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
            encode_wide("Configure Settings...").as_ptr(),
        );

        AppendMenuW(hmenu, MF_SEPARATOR, 0, std::ptr::null());

        // 5. Reload & Exit
        AppendMenuW(
            hmenu,
            MF_STRING,
            ID_TRAY_RELOAD,
            encode_wide("Reload Configuration").as_ptr(),
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

fn handle_window_activated(hwnd: HWND, state: &mut AppState) {
    if hwnd.is_null() || state.desktop_mgr.suppress_foreground {
        return;
    }

    // 1. Resolve to root owner window if needed (e.g. child, dialog, or owned popup)
    let target_hwnd = unsafe {
        let root = GetAncestor(hwnd, GA_ROOTOWNER);
        if !root.is_null() && desktop::is_valid_window(root) {
            root
        } else {
            hwnd
        }
    };

    if !desktop::is_valid_window(target_hwnd) {
        return;
    }

    // 2. Find tracked location of target window (or original hwnd as fallback)
    let (mon_idx, desk_idx) = match state.desktop_mgr.find_window(target_hwnd) {
        Some(loc) => loc,
        None => match state.desktop_mgr.find_window(hwnd) {
            Some(loc) => loc,
            None => return,
        },
    };

    // 3. Check suppression timer on that monitor
    let now = unsafe { windows_sys::Win32::System::SystemInformation::GetTickCount() };
    let mon = &mut state.desktop_mgr.monitors[mon_idx];
    if mon.suppress_foreground_until != 0 {
        if (now as i32).wrapping_sub(mon.suppress_foreground_until as i32) < 0 {
            return;
        }
        mon.suppress_foreground_until = 0;
    }

    // 4. If window is already on the active space of that monitor, nothing to switch
    if mon.current == desk_idx {
        return;
    }

    log_info!(
        "Window activation for {:?} -> Switching Display {} from Space {} to Space {}",
        target_hwnd,
        mon_idx + 1,
        mon.current + 1,
        desk_idx + 1
    );

    // 5. Perform the desktop switch on that monitor and update tray icon
    state.desktop_mgr.suppress_foreground = true;
    state
        .desktop_mgr
        .switch_desktop(mon_idx, desk_idx, Some(target_hwnd));
    state.desktop_mgr.suppress_foreground = false;
    update_state_tray_icon(state);
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
    if id_object != 0 || id_child != 0 || hwnd.is_null() {
        return;
    }

    let mut class_buf = [0u16; 256];
    let len = windows_sys::Win32::UI::WindowsAndMessaging::GetClassNameW(
        hwnd,
        class_buf.as_mut_ptr(),
        256,
    );
    let class_name = if len > 0 {
        String::from_utf16_lossy(&class_buf[..len as usize])
    } else {
        String::new()
    };

    let mut title_buf = [0u16; 256];
    let tlen = windows_sys::Win32::UI::WindowsAndMessaging::GetWindowTextW(
        hwnd,
        title_buf.as_mut_ptr(),
        256,
    );
    let title = if tlen > 0 {
        String::from_utf16_lossy(&title_buf[..tlen as usize])
    } else {
        String::new()
    };

    let is_task_view = class_name == "MultitaskingViewHost"
        || class_name == "XamlExplorerHost"
        || (class_name == "Windows.UI.Core.CoreWindow"
            && (title == "Task View"
                || title == "Vista de tareas"
                || title == "MultitaskingView"
                || title.contains("Task View")));

    if is_task_view {
        log_info!(
            "Intercepted native Windows Task View window (hwnd: {:?}, class: '{}', title: '{}')",
            hwnd,
            class_name,
            title
        );
        with_app_state(|state| {
            if state.config.intercept_win_tab {
                // Dismiss native Task View
                windows_sys::Win32::UI::Input::KeyboardAndMouse::keybd_event(
                    windows_sys::Win32::UI::Input::KeyboardAndMouse::VK_ESCAPE as u8,
                    0,
                    0,
                    0,
                );
                windows_sys::Win32::UI::Input::KeyboardAndMouse::keybd_event(
                    windows_sys::Win32::UI::Input::KeyboardAndMouse::VK_ESCAPE as u8,
                    0,
                    windows_sys::Win32::UI::Input::KeyboardAndMouse::KEYEVENTF_KEYUP,
                    0,
                );
                mission_control::show_mission_control(state);
            }
        });
        return;
    }

    with_app_state(|state| {
        handle_window_activated(hwnd, state);
    });
}

fn update_foreground_hook(state: &mut AppState) {
    if state._win_event_hook.is_none() {
        state._win_event_hook = WinEventHook::install(foreground_hook_proc);
    }
}
