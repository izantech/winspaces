#![windows_subsystem = "windows"]

mod desktop;
mod hooks;
mod hotkeys;
mod layout_store;
mod logger;
mod menu;
mod mission_control;
mod settings_ui;
mod shell_cloak;
mod topology;
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
    GetAncestor, GetCursorPos, GetMessageW, KillTimer, PostQuitMessage, RegisterClassW,
    RegisterShellHookWindow, RegisterWindowMessageW, SetForegroundWindow, SetTimer, TrackPopupMenu,
    TranslateMessage, GA_ROOTOWNER, HSHELL_WINDOWACTIVATED, HSHELL_WINDOWCREATED, MF_CHECKED,
    MF_DISABLED, MF_POPUP, MF_SEPARATOR, MF_STRING, MF_UNCHECKED, MSG, TPM_RIGHTBUTTON,
    WM_ENDSESSION, WM_TIMER, WM_WTSSESSION_CHANGE, WNDCLASSW, WS_EX_TOOLWINDOW, WS_POPUP,
};
use winspaces_common::{
    Config, LayoutStore, TopologySnapshot, WINSPACES_MSG_WINDOW_CLASS, WINSPACES_MSG_WINDOW_TITLE,
    WM_WINSPACES_CAPTURE_WORKSPACE, WM_WINSPACES_RELOAD_CONFIG, WM_WINSPACES_RESTORE_WORKSPACE,
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
const ID_TRAY_CHECK_UPDATES: usize = 1006;
const ID_TRAY_SWITCH_BASE: usize = 2000;
// Reserved offsets inside each monitor's 100-wide command stride
// (`ID_TRAY_SWITCH_BASE + mon_idx * 100 + offset`). Space indices only ever
// reach MAX_DESKTOPS - 1 = 8, so 98/99 can never collide with a switch.
const TRAY_OFFSET_ADD_SPACE: usize = 98;
const TRAY_OFFSET_REMOVE_SPACE: usize = 99;

const TIMER_RECONCILE: usize = 1;
const TIMER_SNAPSHOT: usize = 2;
const TIMER_PERSIST: usize = 3;

/// A topology change arrives as a burst of `WM_DISPLAYCHANGE` messages while
/// the OS is still reflowing windows. Wait for the dust to settle, then
/// reconcile once.
const RECONCILE_DEBOUNCE_MS: u32 = 1200;
/// How often the live layout is re-shadowed. `WM_DISPLAYCHANGE` fires *after*
/// windows have already been reflowed, so the layout worth restoring has to be
/// recorded continuously, before anything goes wrong.
const SNAPSHOT_INTERVAL_MS: u32 = 5000;
/// Layout changes constantly; the disk does not need to hear about every drag.
/// Kept short because a daemon restart or crash inside the window loses the
/// arrangement — a 30 s debounce once replayed a four-minute-old snapshot on
/// boot, reverting spaces the user had since rearranged.
const PERSIST_DEBOUNCE_MS: u32 = 5_000;

// Session-change reasons for WM_WTSSESSION_CHANGE (not exposed by windows-sys).
const WTS_CONSOLE_CONNECT: usize = 0x1;
const WTS_CONSOLE_DISCONNECT: usize = 0x2;
const WTS_REMOTE_CONNECT: usize = 0x3;
const WTS_REMOTE_DISCONNECT: usize = 0x4;

/// Manual update affordance: the tray item opens the releases page in the
/// default browser. No network code lives in the daemon.
const UPDATE_URL: &str = "https://github.com/izantech/winspaces/releases/latest";

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
    /// Persisted layouts, one per topology signature.
    layouts: LayoutStore,
    /// Live layout for the current topology, refreshed on a timer. This is what
    /// gets promoted into `layouts` — capturing only at `WM_DISPLAYCHANGE`
    /// would record the damage, not the layout worth restoring.
    shadow: Option<TopologySnapshot>,
    shadow_dirty: bool,
    /// Signature of the topology the last reconcile settled on.
    last_signature: String,
}

fn enable_menu_theming() {
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
        // While the custom tray menu is open it owns the keyboard: navigation
        // keys are re-posted to the menu window and swallowed here (the menu
        // never activates, so no window has focus to receive them natively).
        if menu::is_menu_open() && menu::forward_key(kbd.vkCode) {
            return 1;
        }
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

/// Record panics to the log before the process dies.
///
/// The release profile builds with `panic = "abort"` and the binary is
/// `windows_subsystem = "windows"`, so a panic produces no console output, no
/// dialog, and frequently no Application Error event — the daemon simply
/// vanishes mid-session with the log ending on an unrelated line. The hook
/// still runs before the abort, which is the only chance to say what happened.
fn install_panic_logger() {
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
    install_panic_logger();

    let args: Vec<String> = std::env::args().collect();
    if args.len() > 1 && args[1] == "--settings" {
        // The settings window runs as its own process instance of this exe;
        // none of the daemon machinery below is initialized for it.
        log_info!("Starting WinSpaces settings window...");
        settings_ui::run_settings();
        return;
    }

    // Control flags are handled before any daemon initialization: they are
    // short-lived invocations of the same exe, and running the daemon's setup
    // for them wrote a misleading "Starting WinSpaces daemon" banner into the
    // shared log on every diagnostic run.
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
        // No DesktopManager here: it is a diagnostic that may run alongside a
        // live daemon, and `DesktopManager::new` un-cloaks that daemon's hidden
        // windows via `reclaim_orphaned_windows`.
        unsafe {
            workspaces::dump_all_window_metrics(out_file);
        }
        log_info!("Wrote window dump to {}", out_file);
        return;
    }

    log_info!("Starting WinSpaces daemon (v0.1.0)...");
    enable_menu_theming();
    unsafe {
        windows_sys::Win32::System::Com::CoInitializeEx(
            null_mut(),
            windows_sys::Win32::System::Com::COINIT_APARTMENTTHREADED as _,
        );
    }
    mission_control::init_mission_control();

    let config_path = Config::get_config_path();
    let config = Config::load_from_file(&config_path);

    // Single-instance guard: autostart can be wired through both the HKCU Run
    // key and the elevated scheduled task; a second daemon would double-cloak
    // every managed window.
    unsafe {
        if !find_daemon_window().is_null() {
            log_warn!("Another WinSpaces daemon is already running; exiting");
            return;
        }
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

        // RDP connect/disconnect swaps the whole display topology; the session
        // notification is the earliest warning that it is about to happen.
        if windows_sys::Win32::System::RemoteDesktop::WTSRegisterSessionNotification(
            hwnd,
            windows_sys::Win32::System::RemoteDesktop::NOTIFY_FOR_THIS_SESSION,
        ) == 0
        {
            log_warn!("WTSRegisterSessionNotification failed; relying on WM_DISPLAYCHANGE alone");
        }

        let mut desktop_mgr = DesktopManager::new();
        desktop_mgr.show_all_taskbar = config.show_all_taskbar;
        log_info!(
            "Initialized DesktopManager with {} monitors detected",
            desktop_mgr.monitors.len()
        );

        let tray_icon = TrayIcon::new(hwnd);
        let win_event_hook = WinEventHook::install(foreground_hook_proc);

        let keyboard_hook = KeyboardHook::install(Some(low_level_keyboard_proc));

        let layouts = LayoutStore::load_from_file(&LayoutStore::get_path());
        let signature = desktop_mgr.topology_signature();
        log_info!(
            "Startup topology [{}]; {} stored layout(s)",
            signature,
            layouts.topologies.len()
        );

        let mut state = AppState {
            config: config.clone(),
            desktop_mgr,
            tray_icon,
            _win_event_hook: win_event_hook,
            _keyboard_hook: keyboard_hook,
            message_hwnd: hwnd,
            shell_hook_msg,
            layouts,
            shadow: None,
            shadow_dirty: false,
            last_signature: signature.clone(),
        };

        if config.auto_restore_workspaces && !config.workspace_rules.is_empty() {
            log_info!("Auto-restoring workspace window rules on startup...");
            restore_workspace_rules(&mut state);
        }

        // Space counts are structural, not layout: apply them from the stored
        // snapshot even when auto-restore is off, so a daemon restart doesn't
        // collapse every monitor back to the default four spaces.
        if let Some(snapshot) = state.layouts.find(&signature).cloned() {
            layout_store::apply_space_counts(&mut state.desktop_mgr, &snapshot);
        }

        // A daemon restart is itself a layout loss: spaces live only in memory,
        // so every window was just re-scanned onto space 1. Replaying the stored
        // layout for this topology puts them back.
        if config.auto_restore_workspaces && !topology::is_remote_session() {
            if let Some(snapshot) = state.layouts.find(&signature).cloned() {
                log_info!("Restoring stored layout for startup topology");
                layout_store::restore_snapshot(&mut state.desktop_mgr, &snapshot);
            }
        }

        let startup_max_spaces = state.desktop_mgr.max_space_count();
        APP_STATE.with(|s| *s.borrow_mut() = Some(state));

        SetTimer(hwnd, TIMER_SNAPSHOT, SNAPSHOT_INTERVAL_MS, None);

        if !HotkeyManager::register_all(&config, startup_max_spaces) {
            log_warn!("Hotkey registration failed at startup; opening settings window.");
            with_app_state(|state| {
                state.desktop_mgr.handle_hotkeys = false;
            });
            launch_settings();
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
        KillTimer(hwnd, TIMER_SNAPSHOT);
        windows_sys::Win32::System::RemoteDesktop::WTSUnRegisterSessionNotification(hwnd);
        with_app_state(|state| {
            persist_shadow(state);
            state.desktop_mgr.windows_show_all();
            state.tray_icon.remove();
        });
    }
}

/// Settle a display-topology change: rebuild the monitor table, then replay the
/// stored layout if this topology is one we have seen before.
///
/// Runs once per burst, on the debounce timer rather than inline in
/// `WM_DISPLAYCHANGE`, because RDP connect/disconnect emits several of those
/// while the OS is still moving windows around.
fn reconcile_topology(state: &mut AppState) {
    state.desktop_mgr.handle_display_change();
    state.desktop_mgr.reconcile_pending = false;

    let signature = state.desktop_mgr.topology_signature();
    let remote = topology::is_remote_session();
    if signature == state.last_signature {
        log_info!("Topology unchanged after settle [{}]", signature);
        return;
    }
    log_info!(
        "Topology settled: [{}] -> [{}] (remote session: {})",
        state.last_signature,
        signature,
        remote
    );
    state.last_signature = signature.clone();
    // The shadow described the *previous* topology; drop it so the next tick
    // captures this one from scratch instead of diffing against stale data.
    state.shadow = None;

    if remote {
        // The physical monitors are detached and the RDP virtual display owns
        // the desktop. Whatever the layout looks like here is disposable — and
        // because it carries its own signature it can never overwrite the desk
        // layout on disk.
        log_info!("Remote session active; layout shadowing paused");
        return;
    }

    match state.layouts.find(&signature).cloned() {
        Some(snapshot) => layout_store::restore_snapshot(&mut state.desktop_mgr, &snapshot),
        None => {
            log_info!(
                "No stored layout for [{}]; leaving windows where the OS put them",
                signature
            );
        }
    }
}

/// Re-shadow the live layout. Cheap enough to run on a timer: one `EnumWindows`
/// pass over the tracked set.
fn shadow_tick(state: &mut AppState) {
    // Never shadow mid-transition: a capture taken while the OS is still moving
    // windows would promote the scramble into the stored reference layout.
    if state.desktop_mgr.reconcile_pending
        || state.desktop_mgr.is_settling()
        || topology::is_remote_session()
    {
        return;
    }

    let snapshot = layout_store::capture_snapshot(&state.desktop_mgr);
    // An empty capture means the scan raced a teardown; never promote it over a
    // good layout.
    if snapshot.windows.is_empty() {
        return;
    }
    if state
        .shadow
        .as_ref()
        .is_some_and(|prev| layout_store::same_layout(prev, &snapshot))
    {
        return;
    }

    state.shadow = Some(snapshot);
    state.shadow_dirty = true;
    unsafe {
        SetTimer(state.message_hwnd, TIMER_PERSIST, PERSIST_DEBOUNCE_MS, None);
    }
}

fn persist_shadow(state: &mut AppState) {
    unsafe {
        KillTimer(state.message_hwnd, TIMER_PERSIST);
    }
    if !state.shadow_dirty {
        return;
    }
    let Some(snapshot) = state.shadow.clone() else {
        return;
    };
    let windows = snapshot.windows.len();
    let signature = snapshot.signature.clone();
    state.layouts.upsert(snapshot);
    match state.layouts.save_to_file(&LayoutStore::get_path()) {
        Ok(()) => {
            state.shadow_dirty = false;
            log_info!("Saved layout for [{}]: {} windows", signature, windows);
        }
        Err(e) => {
            log_error!("Failed to save layouts.json: {}", e);
        }
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
    state.desktop_mgr.begin_settle(layout_store::SETTLE_MS);
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
        // Aim at the monitor the rule names, not at whatever currently covers
        // the saved coordinates. After a topology change those coordinates can
        // point at a different display entirely.
        let target = state
            .desktop_mgr
            .monitors
            .get(rule.display_index)
            .map(|m| m.hmon);
        workspaces::apply_rule_to_window(hwnd, &rule, target);
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
                    log_info!("Tray menu: Open Settings requested");
                    launch_settings();
                } else if cmd == ID_TRAY_CHECK_UPDATES {
                    log_info!("Tray menu: Check for updates requested");
                    let verb = encode_wide("open");
                    let url = encode_wide(UPDATE_URL);
                    windows_sys::Win32::UI::Shell::ShellExecuteW(
                        null_mut(),
                        verb.as_ptr(),
                        url.as_ptr(),
                        std::ptr::null(),
                        std::ptr::null(),
                        windows_sys::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL,
                    );
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
                        let _ = HotkeyManager::register_all(
                            &state.config,
                            state.desktop_mgr.max_space_count(),
                        );
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
                    match offset % 100 {
                        TRAY_OFFSET_ADD_SPACE => {
                            log_info!("Tray menu: New space on Monitor {}", mon_idx + 1);
                            with_app_state(|state| add_space_on(state, mon_idx));
                        }
                        TRAY_OFFSET_REMOVE_SPACE => {
                            log_info!("Tray menu: Remove last space on Monitor {}", mon_idx + 1);
                            with_app_state(|state| {
                                // The tray removes the *last* space; targeted
                                // removal is Mission Control's close button.
                                let count = state
                                    .desktop_mgr
                                    .monitors
                                    .get(mon_idx)
                                    .map(|m| m.desktops.len())
                                    .unwrap_or(0);
                                if count > 1 {
                                    remove_space_on(state, mon_idx, count - 1);
                                }
                            });
                        }
                        desk_idx => {
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
                    }
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
                    if !HotkeyManager::register_all(
                        &state.config,
                        state.desktop_mgr.max_space_count(),
                    ) {
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
            windows_sys::Win32::UI::WindowsAndMessaging::WM_ACTIVATE => {
                // The custom tray menu never activates; this hidden window is
                // made foreground instead when the menu opens. Losing that
                // foreground status (Alt-Tab, click into another app) is one
                // of the menu's light-dismiss signals.
                if (wparam & 0xffff) as u32
                    == windows_sys::Win32::UI::WindowsAndMessaging::WA_INACTIVE
                {
                    menu::handle_owner_deactivate();
                }
                0
            }
            windows_sys::Win32::UI::WindowsAndMessaging::WM_DISPLAYCHANGE => {
                // Debounced: a dock, undock or RDP transition fires several of
                // these while the OS is still relocating windows. Acting on the
                // first one records a half-finished desktop.
                log_info!("Display topology changed; scheduling reconcile");
                with_app_state(|state| {
                    state.desktop_mgr.reconcile_pending = true;
                });
                SetTimer(hwnd, TIMER_RECONCILE, RECONCILE_DEBOUNCE_MS, None);
                0
            }

            WM_WTSSESSION_CHANGE => {
                let reason = match wparam {
                    WTS_CONSOLE_CONNECT => "console connect",
                    WTS_CONSOLE_DISCONNECT => "console disconnect",
                    WTS_REMOTE_CONNECT => "remote connect",
                    WTS_REMOTE_DISCONNECT => "remote disconnect",
                    _ => "other",
                };
                log_info!("Session change: {} ({})", reason, wparam);
                if matches!(
                    wparam,
                    WTS_CONSOLE_CONNECT
                        | WTS_CONSOLE_DISCONNECT
                        | WTS_REMOTE_CONNECT
                        | WTS_REMOTE_DISCONNECT
                ) {
                    // The display swap that accompanies an RDP transition can
                    // land either side of this message, so join the same
                    // debounce rather than reconciling here.
                    with_app_state(|state| {
                        state.desktop_mgr.reconcile_pending = true;
                    });
                    SetTimer(hwnd, TIMER_RECONCILE, RECONCILE_DEBOUNCE_MS, None);
                }
                0
            }

            WM_TIMER => {
                match wparam {
                    TIMER_RECONCILE => {
                        KillTimer(hwnd, TIMER_RECONCILE);
                        with_app_state(|state| {
                            reconcile_topology(state);
                            update_state_tray_icon(state);
                        });
                    }
                    TIMER_SNAPSHOT => with_app_state(shadow_tick),
                    TIMER_PERSIST => with_app_state(persist_shadow),
                    _ => {}
                }
                0
            }

            WM_ENDSESSION => {
                // Logoff/shutdown previously skipped cleanup entirely, leaving
                // windows cloaked for the next session to reclaim. Save the
                // layout and un-hide everything while there is still time.
                if wparam != 0 {
                    log_info!("Session ending; persisting layout and restoring windows");
                    with_app_state(|state| {
                        persist_shadow(state);
                        state.desktop_mgr.windows_show_all();
                    });
                }
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
                                    let target = state
                                        .desktop_mgr
                                        .monitors
                                        .get(rule.display_index)
                                        .map(|m| m.hmon);
                                    workspaces::apply_rule_to_window(target_hwnd, &rule, target);
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
        let mut desktop_changed = false;
        if (HOTKEY_ID_SWITCH_BASE..HOTKEY_ID_MOVE_BASE).contains(&id) {
            let desk = (id - HOTKEY_ID_SWITCH_BASE) as usize;
            state.desktop_mgr.go_to_desk(desk);
            update_state_tray_icon(state);
            desktop_changed = true;
        } else if (HOTKEY_ID_MOVE_BASE..HOTKEY_ID_SPECIAL_BASE).contains(&id) {
            let desk = (id - HOTKEY_ID_MOVE_BASE) as usize;
            state.desktop_mgr.move_to_desk(desk);
            update_state_tray_icon(state);
            desktop_changed = true;
        } else if id == HOTKEY_ID_PREV {
            state.desktop_mgr.step_desktop(-1);
            update_state_tray_icon(state);
            desktop_changed = true;
        } else if id == HOTKEY_ID_NEXT {
            state.desktop_mgr.step_desktop(1);
            update_state_tray_icon(state);
            desktop_changed = true;
        } else if id == HOTKEY_ID_MOVE_PREV {
            state.desktop_mgr.step_move_window(-1);
            update_state_tray_icon(state);
            desktop_changed = true;
        } else if id == HOTKEY_ID_MOVE_NEXT {
            state.desktop_mgr.step_move_window(1);
            update_state_tray_icon(state);
            desktop_changed = true;
        } else if id == HOTKEY_ID_TOGGLE {
            toggle_hotkeys(state);
        } else if id == HOTKEY_ID_MISSION_CONTROL {
            mission_control::toggle_mission_control(state);
        }

        // Global switch/move hotkeys pressed with the overlay open should
        // update it in place, never dismiss it.
        if desktop_changed && mission_control::is_mission_control_active() {
            mission_control::refresh_mission_control(state);
        }
    });
}

fn toggle_hotkeys(state: &mut AppState) {
    state.desktop_mgr.handle_hotkeys = !state.desktop_mgr.handle_hotkeys;
    if state.desktop_mgr.handle_hotkeys {
        if !HotkeyManager::register_all(&state.config, state.desktop_mgr.max_space_count()) {
            log_warn!("Hotkey re-registration failed upon toggle; opening settings window.");
            launch_settings();
            state.desktop_mgr.handle_hotkeys = false;
        }
    } else {
        HotkeyManager::unregister_all();
    }
}

/// Single choke points for changing a monitor's space count: tray and Mission
/// Control both land here, so persistence, hotkey registration, the tray badge
/// and an open overlay can never drift apart.
fn add_space_on(state: &mut AppState, mon_idx: usize) {
    let old_max = state.desktop_mgr.max_space_count();
    if state.desktop_mgr.add_space(mon_idx) {
        after_space_count_change(state, old_max);
    }
}

fn remove_space_on(state: &mut AppState, mon_idx: usize, desk_idx: usize) {
    let old_max = state.desktop_mgr.max_space_count();
    if state.desktop_mgr.remove_space(mon_idx, desk_idx) {
        after_space_count_change(state, old_max);
    }
}

/// Choke point for moving a space within a monitor — Mission Control's card
/// drag and its `Ctrl+Shift+←/→` equivalent both land here. Unlike add/remove
/// the space count is unchanged, so there are no digit hotkeys to re-register
/// and no count to persist; the windows travel with the space, so the next
/// `shadow_tick` capture writes their new `space_index` values to disk.
fn reorder_space_on(state: &mut AppState, mon_idx: usize, from_idx: usize, to_idx: usize) {
    if state.desktop_mgr.reorder_space(mon_idx, from_idx, to_idx) {
        update_state_tray_icon(state);
        if mission_control::is_mission_control_active() {
            mission_control::refresh_mission_control(state);
        }
    }
}

fn after_space_count_change(state: &mut AppState, old_max: usize) {
    persist_space_counts(state);
    let new_max = state.desktop_mgr.max_space_count();
    if new_max != old_max && state.desktop_mgr.handle_hotkeys {
        HotkeyManager::unregister_all();
        if !HotkeyManager::register_all(&state.config, new_max) {
            log_warn!("Hotkey re-registration failed after space count change.");
        }
    }
    update_state_tray_icon(state);
    if mission_control::is_mission_control_active() {
        mission_control::refresh_mission_control(state);
    }
}

/// Write the live per-monitor space counts straight into the stored topology
/// entry. The shadow path cannot be relied on for this: `shadow_tick` refuses
/// empty captures, so a count change with no windows open would never reach
/// disk. Cheap and user-initiated, so no debounce.
fn persist_space_counts(state: &mut AppState) {
    let signature = state.desktop_mgr.topology_signature();
    let live = layout_store::live_monitors(&state.desktop_mgr);

    if let Some(entry) = state
        .layouts
        .topologies
        .iter_mut()
        .find(|t| t.signature == signature)
    {
        for mon in &mut entry.monitors {
            if let Some(live_mon) = live.iter().find(|l| l.stable_id == mon.stable_id) {
                mon.space_count = live_mon.space_count;
            }
        }
    } else {
        state.layouts.upsert(winspaces_common::TopologySnapshot {
            signature: signature.clone(),
            monitors: live.clone(),
            windows: Vec::new(),
            captured_unix: winspaces_common::unix_now(),
        });
    }

    if let Err(e) = state.layouts.save_to_file(&LayoutStore::get_path()) {
        log_error!(
            "Failed to save layouts.json after space count change: {}",
            e
        );
    }

    // Mirror the counts into the shadow so the next shadow_tick diff doesn't
    // immediately mark it dirty and rewrite the file for the same change.
    if let Some(shadow) = state.shadow.as_mut() {
        if shadow.signature == signature {
            for mon in &mut shadow.monitors {
                if let Some(live_mon) = live.iter().find(|l| l.stable_id == mon.stable_id) {
                    mon.space_count = live_mon.space_count;
                }
            }
        }
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

/// Open the settings window: a separate process instance of this exe, so a
/// settings crash can never take the daemon down.
fn launch_settings() {
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

fn show_tray_menu(hwnd: HWND) {
    unsafe {
        let mut pt = POINT { x: 0, y: 0 };
        GetCursorPos(&mut pt);

        let (show_tb, monitors_info) = APP_STATE.with(|s| {
            s.try_borrow()
                .ok()
                .and_then(|st| {
                    st.as_ref().map(|state| {
                        let show_tb = state.config.show_all_taskbar;
                        let mons: Vec<(usize, usize, usize)> = state
                            .desktop_mgr
                            .monitors
                            .iter()
                            .enumerate()
                            .map(|(idx, m)| (idx, m.current, m.desktops.len()))
                            .collect();
                        (show_tb, mons)
                    })
                })
                .unwrap_or((true, vec![(0, 0, winspaces_common::DEFAULT_DESKTOPS)]))
        });

        // Windows 11 gets the custom acrylic menu; older builds keep the
        // classic dark system HMENU.
        if menu::win_build() >= 22000 {
            menu::show_menu(hwnd, build_menu_entries(show_tb, &monitors_info), pt);
        } else {
            show_tray_menu_legacy(hwnd, pt, show_tb, &monitors_info);
        }
    }
}

fn build_menu_entries(
    show_tb: bool,
    monitors_info: &[(usize, usize, usize)],
) -> Vec<menu::MenuEntry> {
    use menu::{MenuEntry, MenuItemData};
    fn item(
        id: usize,
        glyph: Option<u16>,
        label: &str,
        shortcut: Option<String>,
        checked: bool,
        submenu: Option<Vec<MenuEntry>>,
    ) -> MenuEntry {
        MenuEntry::Item(MenuItemData {
            id,
            glyph,
            label: label.to_string(),
            shortcut,
            checked,
            submenu,
        })
    }

    let mut entries = vec![
        MenuEntry::Header(format!("WinSpaces v{}", env!("CARGO_PKG_VERSION"))),
        item(
            ID_TRAY_MISSION_CONTROL,
            Some(menu::GLYPH_TASK_VIEW),
            "Mission Control",
            Some("Win+Tab".to_string()),
            false,
            None,
        ),
        MenuEntry::Separator,
    ];

    for &(mon_idx, curr_space, space_count) in monitors_info {
        let mut sub: Vec<MenuEntry> = (0..space_count)
            .map(|desk_idx| {
                item(
                    ID_TRAY_SWITCH_BASE + mon_idx * 100 + desk_idx,
                    None,
                    &format!("Space {}", desk_idx + 1),
                    Some(format!("Alt+{}", desk_idx + 1)),
                    curr_space == desk_idx,
                    None,
                )
            })
            .collect();
        sub.push(MenuEntry::Separator);
        if space_count < winspaces_common::MAX_DESKTOPS {
            sub.push(item(
                ID_TRAY_SWITCH_BASE + mon_idx * 100 + TRAY_OFFSET_ADD_SPACE,
                Some(menu::GLYPH_ADD),
                "New Space",
                None,
                false,
                None,
            ));
        }
        if space_count > 1 {
            sub.push(item(
                ID_TRAY_SWITCH_BASE + mon_idx * 100 + TRAY_OFFSET_REMOVE_SPACE,
                Some(menu::GLYPH_REMOVE),
                &format!("Remove Space {}", space_count),
                None,
                false,
                None,
            ));
        }
        entries.push(item(
            0,
            Some(menu::GLYPH_MONITOR),
            &format!("Display {}", mon_idx + 1),
            Some(format!("Space {}", curr_space + 1)),
            false,
            Some(sub),
        ));
    }

    entries.push(MenuEntry::Separator);
    entries.push(item(
        ID_TRAY_CAPTURE_WS,
        Some(menu::GLYPH_CAMERA),
        "Capture Workspace Layout",
        None,
        false,
        None,
    ));
    entries.push(item(
        ID_TRAY_RESTORE_WS,
        Some(menu::GLYPH_RESTORE),
        "Restore Workspace Layout",
        None,
        false,
        None,
    ));
    entries.push(MenuEntry::Separator);
    entries.push(item(
        ID_TRAY_TOGGLE_TASKBAR,
        None,
        "Show all windows on taskbar",
        None,
        show_tb,
        None,
    ));
    entries.push(item(
        ID_TRAY_CONFIG,
        Some(menu::GLYPH_SETTINGS),
        "Settings",
        None,
        false,
        None,
    ));
    entries.push(item(
        ID_TRAY_CHECK_UPDATES,
        Some(menu::GLYPH_SYNC),
        "Check for Updates",
        None,
        false,
        None,
    ));
    entries.push(MenuEntry::Separator);
    entries.push(item(
        ID_TRAY_RELOAD,
        Some(menu::GLYPH_REFRESH),
        "Reload Configuration",
        None,
        false,
        None,
    ));
    entries.push(item(
        ID_TRAY_EXIT,
        Some(menu::GLYPH_CLOSE),
        "Exit WinSpaces",
        None,
        false,
        None,
    ));
    entries
}

fn show_tray_menu_legacy(
    hwnd: HWND,
    pt: POINT,
    show_tb: bool,
    monitors_info: &[(usize, usize, usize)],
) {
    unsafe {
        let hmenu = CreatePopupMenu();

        // 1. Header item showing overall status
        let mut status_str = format!("WinSpaces v{}", env!("CARGO_PKG_VERSION"));
        for (i, (_, curr, _)) in monitors_info.iter().enumerate() {
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
        for (mon_idx, curr_space, space_count) in monitors_info {
            let hsub = CreatePopupMenu();
            for desk_idx in 0..*space_count {
                let cmd_id = ID_TRAY_SWITCH_BASE + (mon_idx * 100) + desk_idx;
                let flags = if *curr_space == desk_idx {
                    MF_CHECKED | MF_STRING
                } else {
                    MF_UNCHECKED | MF_STRING
                };
                let label = format!("Space {}  (Alt+{})", desk_idx + 1, desk_idx + 1);
                AppendMenuW(hsub, flags, cmd_id, encode_wide(&label).as_ptr());
            }
            AppendMenuW(hsub, MF_SEPARATOR, 0, std::ptr::null());
            if *space_count < winspaces_common::MAX_DESKTOPS {
                AppendMenuW(
                    hsub,
                    MF_STRING,
                    ID_TRAY_SWITCH_BASE + (mon_idx * 100) + TRAY_OFFSET_ADD_SPACE,
                    encode_wide("New Space").as_ptr(),
                );
            }
            if *space_count > 1 {
                AppendMenuW(
                    hsub,
                    MF_STRING,
                    ID_TRAY_SWITCH_BASE + (mon_idx * 100) + TRAY_OFFSET_REMOVE_SPACE,
                    encode_wide(&format!("Remove Space {}", space_count)).as_ptr(),
                );
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
        AppendMenuW(
            hmenu,
            MF_STRING,
            ID_TRAY_CHECK_UPDATES,
            encode_wide("Check for Updates...").as_ptr(),
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
        // Documented tray-menu quirk (KB135788): without a posted no-op the
        // menu won't dismiss on the first click outside it.
        windows_sys::Win32::UI::WindowsAndMessaging::PostMessageW(
            hwnd,
            windows_sys::Win32::UI::WindowsAndMessaging::WM_NULL,
            0,
            0,
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
