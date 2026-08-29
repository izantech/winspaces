#![windows_subsystem = "windows"]

mod app;
mod elevation;
mod handlers;
mod hostfns;
mod restore;
mod shadow;
mod spaces;
mod tray_menu;
mod wndproc;

use std::ptr::null_mut;
use std::sync::atomic::{AtomicIsize, Ordering};
use windows_sys::Win32::Foundation::HWND;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    ChangeWindowMessageFilterEx, CreateWindowExW, DispatchMessageW, GetMessageW, KillTimer,
    RegisterClassW, RegisterShellHookWindow, RegisterWindowMessageW, SetCoalescableTimer, SetTimer,
    TranslateMessage, MSG, MSGFLT_ALLOW, WM_COMMAND, WM_HOTKEY, WNDCLASSW, WS_EX_TOOLWINDOW,
    WS_POPUP,
};
use winspaces_common::logger::Logger;
use winspaces_common::{
    log_info, log_warn, Config, LayoutStore, WINSPACES_MSG_WINDOW_CLASS,
    WINSPACES_MSG_WINDOW_TITLE, WM_WINSPACES_CAPTURE_WORKSPACE, WM_WINSPACES_RELOAD_CONFIG,
    WM_WINSPACES_RESTORE_WORKSPACE, WM_WINSPACES_RETILE, WM_WINSPACES_TILING_TOGGLE,
    WM_WINSPACES_TOGGLE_MISSION_CONTROL,
};
use winspaces_core::hotkeys::HotkeyManager;
use winspaces_core::spaces::SpaceManager;
use winspaces_core::{layout_store, topology, workspaces};
use winspaces_ui::tray::TrayIcon;
use winspaces_ui::{mission_control, settings, space_indicator};
use winspaces_win32::hooks::{KeyboardHook, WinEventHook};
use winspaces_win32::module::app_instance;
use winspaces_win32::text::encode_wide;

use app::{
    enable_menu_theming, find_daemon_window, install_panic_logger, launch_settings,
    update_tray_icon, with_app_state, AppState, APP_STATE,
};
use handlers::commands::ID_TRAY_EXIT;
use handlers::session::{
    RESTORE_VERIFY_MS, SNAPSHOT_INTERVAL_MS, TIMER_RESTORE_VERIFY, TIMER_SNAPSHOT,
};
use restore::restore_workspace_rules;
use shadow::persist_shadow;
use spaces::handle_hotkey;
use wndproc::wndproc;

static DAEMON_HWND: AtomicIsize = AtomicIsize::new(0);

fn schedule_retile_post() {
    let hwnd_val = DAEMON_HWND.load(Ordering::Acquire);
    if hwnd_val != 0 {
        unsafe {
            windows_sys::Win32::UI::WindowsAndMessaging::PostMessageW(
                hwnd_val as HWND,
                WM_WINSPACES_RETILE,
                0,
                0,
            );
        }
    } else {
        log_warn!("schedule_retile_post: DAEMON_HWND is not initialized");
    }
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
        settings::run_settings();
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
                    WM_COMMAND,
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
    if args.len() > 1 && (args[1] == "--tiling-toggle" || args[1] == "-t") {
        unsafe {
            let hwnd = find_daemon_window();
            if !hwnd.is_null() {
                windows_sys::Win32::UI::WindowsAndMessaging::PostMessageW(
                    hwnd,
                    WM_WINSPACES_TILING_TOGGLE,
                    0,
                    0,
                );
            } else {
                log_warn!("--tiling-toggle requested but no running daemon was found");
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
        // No SpaceManager here: it is a diagnostic that may run alongside a
        // live daemon, and `SpaceManager::new` un-cloaks that daemon's hidden
        // windows via `reclaim_orphaned_windows`.
        unsafe {
            workspaces::dump_all_window_metrics(out_file);
        }
        log_info!("Wrote window dump to {}", out_file);
        return;
    }

    if args.len() > 1 && (args[1] == "--enable-elevation" || args[1] == "--elevate-enable") {
        elevation::handle_enable_elevation();
        return;
    }
    if args.len() > 1 && (args[1] == "--disable-elevation" || args[1] == "--elevate-disable") {
        elevation::handle_disable_elevation();
        return;
    }
    if args.len() > 1 && (args[1] == "--elevation-status" || args[1] == "--status-elevation") {
        elevation::handle_elevation_status();
        return;
    }
    if args.len() > 1
        && (args[1] == "--restart" || args[1] == "--restart-daemon" || args[1] == "-r")
    {
        elevation::handle_restart_daemon();
        return;
    }

    // The early AttachConsole ties this process to the launching terminal's
    // console group; a console teardown (terminal closed, session disconnect,
    // hibernation) then ExitProcess()es the daemon with no teardown. The CLI
    // paths above want the console; the long-lived daemon must not keep it.
    unsafe {
        windows_sys::Win32::System::Console::FreeConsole();
        windows_sys::Win32::System::Console::SetStdHandle(
            windows_sys::Win32::System::Console::STD_INPUT_HANDLE,
            std::ptr::null_mut(),
        );
        windows_sys::Win32::System::Console::SetStdHandle(
            windows_sys::Win32::System::Console::STD_OUTPUT_HANDLE,
            std::ptr::null_mut(),
        );
        windows_sys::Win32::System::Console::SetStdHandle(
            windows_sys::Win32::System::Console::STD_ERROR_HANDLE,
            std::ptr::null_mut(),
        );
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
    // Before any overlay gesture can fire: Mission Control routes every action
    // that touches daemon state through this table.
    mission_control::install_host(&hostfns::MC_HOST);
    // Same inversion, one level down: `winspaces-core` owns the only place a
    // space actually changes but sits below every UI crate, so the bin — the
    // only crate that can name both sides — hands the indicator down as a
    // plain fn pointer.
    winspaces_core::spaces::set_switch_observer(space_indicator::on_space_switch);
    winspaces_core::tiling::set_retile_scheduler(schedule_retile_post);

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
        config.switch_spaces.len(),
        config.move_spaces.len(),
        config.workspace_rules.len()
    );

    unsafe {
        let instance = app_instance();
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

        let hwnd: HWND = CreateWindowExW(
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
        DAEMON_HWND.store(hwnd as isize, Ordering::Release);

        let is_elevated = winspaces_win32::security::is_current_process_elevated();
        if is_elevated {
            let prop = encode_wide(winspaces_ui::settings::autostart::PROP_ELEVATED);
            windows_sys::Win32::UI::WindowsAndMessaging::SetPropW(hwnd, prop.as_ptr(), 1 as _);
            log_info!("Daemon running with Administrator privileges (Elevated)");
        } else {
            log_info!("Daemon running with standard user integrity (Non-elevated)");
        }

        // The daemon may run elevated while the GUI/CLI run at medium
        // integrity; UIPI silently drops their messages unless allowed here.
        for msg in [
            WM_WINSPACES_RELOAD_CONFIG,
            WM_WINSPACES_CAPTURE_WORKSPACE,
            WM_WINSPACES_RESTORE_WORKSPACE,
            WM_WINSPACES_TOGGLE_MISSION_CONTROL,
            WM_WINSPACES_RETILE,
            WM_WINSPACES_TILING_TOGGLE,
            WM_COMMAND,
        ] {
            ChangeWindowMessageFilterEx(hwnd, msg, MSGFLT_ALLOW, std::ptr::null_mut());
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

        let mut space_mgr = SpaceManager::new();
        space_mgr.show_all_taskbar = config.show_all_taskbar;
        space_mgr.space_indicator = config.space_indicator;
        space_mgr.tiling_gaps = winspaces_core::tiling::Gaps {
            inner: config.tiling.inner_gap,
            outer: config.tiling.outer_gap,
        };
        space_mgr.set_tiling_enabled(config.tiling.enabled);
        space_mgr.float_rules = config.tiling.float_rules.clone();
        log_info!(
            "Initialized SpaceManager with {} monitors detected (tiling enabled: {})",
            space_mgr.monitors.len(),
            space_mgr.tiling_enabled
        );

        let tray_icon = TrayIcon::new(hwnd);
        let win_event_hook = WinEventHook::install(handlers::shell::foreground_hook_proc);
        let show_hook = WinEventHook::install_range(
            winspaces_win32::hooks::EVENT_OBJECT_SHOW,
            winspaces_win32::hooks::EVENT_OBJECT_SHOW,
            handlers::shell::show_hook_proc,
        );
        let minimize_hook = WinEventHook::install_range(
            winspaces_win32::hooks::EVENT_SYSTEM_MINIMIZESTART,
            winspaces_win32::hooks::EVENT_SYSTEM_MINIMIZEEND,
            handlers::shell::minimize_hook_proc,
        );
        let movesize_hook = WinEventHook::install_range(
            winspaces_win32::hooks::EVENT_SYSTEM_MOVESIZESTART,
            winspaces_win32::hooks::EVENT_SYSTEM_MOVESIZEEND,
            handlers::shell::movesize_hook_proc,
        );
        let location_hook = WinEventHook::install_range(
            winspaces_win32::hooks::EVENT_OBJECT_LOCATIONCHANGE,
            winspaces_win32::hooks::EVENT_OBJECT_LOCATIONCHANGE,
            handlers::shell::location_hook_proc,
        );

        let keyboard_hook = KeyboardHook::install(Some(handlers::shell::low_level_keyboard_proc));

        let layouts = LayoutStore::load_from_file(&LayoutStore::get_path());
        let signature = space_mgr.topology_signature();
        log_info!(
            "Startup topology [{}]; {} stored layout(s)",
            signature,
            layouts.topologies.len()
        );

        let mut state = AppState {
            config: config.clone(),
            space_mgr,
            tray_icon,
            _win_event_hook: win_event_hook,
            _show_hook: show_hook,
            _minimize_hook: minimize_hook,
            _movesize_hook: movesize_hook,
            _location_hook: location_hook,
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
            layout_store::apply_space_counts(&mut state.space_mgr, &snapshot);
        }

        // A daemon restart is itself a layout loss: spaces live only in memory,
        // so every window was just re-scanned onto space 1. Replaying the stored
        // layout for this topology puts them back.
        if config.auto_restore_workspaces && !topology::is_remote_session() {
            if let Some(snapshot) = state.layouts.find(&signature).cloned() {
                log_info!("Restoring stored layout for startup topology");
                layout_store::restore_snapshot(&mut state.space_mgr, &snapshot);
                // Same post-restore sweep as the topology reconcile: anything
                // that moves a restored window in the next few seconds gets
                // pushed back rather than adopted.
                SetTimer(hwnd, TIMER_RESTORE_VERIFY, RESTORE_VERIFY_MS, None);
            }
        }

        let startup_max_spaces = state.space_mgr.max_space_count();
        APP_STATE.with(|s| *s.borrow_mut() = Some(state));

        // Coalescable: the tick only needs to be *recent* (display-topology.md
        // §5), so give the scheduler a second of slack per firing instead of a
        // hard 5 s deadline. Keep the tolerance modest — the persist debounce
        // stacks on top of this interval, and 30 s of accumulated staleness
        // once replayed a stale snapshot on boot (see handlers/session.rs).
        SetCoalescableTimer(hwnd, TIMER_SNAPSHOT, SNAPSHOT_INTERVAL_MS, None, 1000);

        if !HotkeyManager::register_all(&config, startup_max_spaces) {
            log_warn!("Hotkey registration failed at startup; opening settings window.");
            with_app_state(|state| {
                state.space_mgr.handle_hotkeys = false;
            });
            launch_settings();
        }

        update_tray_icon();
        log_info!("Initialization complete. Entering WinMain message loop...");

        let mut msg: MSG = std::mem::zeroed();
        while GetMessageW(&mut msg, null_mut(), 0, 0) > 0 {
            if msg.message == WM_HOTKEY {
                handle_hotkey(msg.wParam as i32);
            } else {
                TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }

        log_info!("Exiting message loop. Cleaning up...");
        DAEMON_HWND.store(0, Ordering::Release);
        HotkeyManager::unregister_all();

        KillTimer(hwnd, TIMER_SNAPSHOT);
        windows_sys::Win32::System::RemoteDesktop::WTSUnRegisterSessionNotification(hwnd);
        with_app_state(|state| {
            persist_shadow(state);
            state.space_mgr.windows_show_all();
            state.tray_icon.remove();
        });
    }
}
