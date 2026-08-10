#![windows_subsystem = "windows"]

use serde::Deserialize;
use std::ptr::null_mut;
use tao::{
    event::{Event, WindowEvent},
    event_loop::{ControlFlow, EventLoop},
    window::WindowBuilder,
};
use windows_sys::Win32::System::Registry::{
    RegCloseKey, RegDeleteValueW, RegOpenKeyExW, RegQueryValueExW, RegSetValueExW,
    HKEY_CURRENT_USER, KEY_READ, KEY_WRITE, REG_SZ,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{FindWindowW, PostMessageW, WM_CLOSE};
use winspaces_common::{
    Config, WINSPACES_DAEMON_EXE, WINSPACES_MSG_WINDOW_CLASS, WINSPACES_MSG_WINDOW_TITLE,
    WM_WINSPACES_RELOAD_CONFIG,
};
use wry::WebViewBuilder;

const HTML_CONTENT: &str = include_str!("ui/index.html");
const REG_RUN_PATH: &str = "Software\\Microsoft\\Windows\\CurrentVersion\\Run";
const REG_APP_NAME: &str = "WinSpaces";

#[derive(Debug, Deserialize)]
struct IpcMessage {
    cmd: String,
    config: Option<Config>,
    autostart: Option<bool>,
    action: Option<String>,
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let event_loop = EventLoop::new();
    let window = WindowBuilder::new()
        .with_title("WinSpaces Settings")
        .with_inner_size(tao::dpi::LogicalSize::new(680.0, 740.0))
        .with_resizable(true)
        .build(&event_loop)?;

    let webview = WebViewBuilder::new()
        .with_html(HTML_CONTENT)
        .with_ipc_handler(move |msg| {
            let req = msg.body();
            handle_ipc_request(req);
        })
        .build(&window)?;

    let config_path = Config::get_config_path();
    let cfg = Config::load_from_file(&config_path);
    let daemon_active = is_daemon_running();
    let autostart = get_autostart_status();

    if let Ok(cfg_json) = serde_json::to_string(&cfg) {
        let script = format!(
            "window.onConfigLoaded({}, {}, {});",
            cfg_json, daemon_active, autostart
        );
        let _ = webview.evaluate_script(&script);
    }

    event_loop.run(move |event, _, control_flow| {
        *control_flow = ControlFlow::Wait;

        if let Event::WindowEvent {
            event: WindowEvent::CloseRequested,
            ..
        } = event
        {
            *control_flow = ControlFlow::Exit;
        }
    });
}

fn handle_ipc_request(body: &str) {
    if let Ok(ipc) = serde_json::from_str::<IpcMessage>(body) {
        match ipc.cmd.as_str() {
            "get_config" => {
                notify_daemon_reload();
            }
            "save_config" => {
                if let Some(cfg) = ipc.config {
                    let path = Config::get_config_path();
                    let _ = cfg.save_to_file(&path);
                    if let Some(autostart) = ipc.autostart {
                        set_autostart_status(autostart);
                    }
                    notify_daemon_reload();
                }
            }
            "get_defaults" => {
                let default_cfg = Config::default();
                let path = Config::get_config_path();
                let _ = default_cfg.save_to_file(&path);
                notify_daemon_reload();
            }
            "daemon_action" => {
                if let Some(action) = ipc.action {
                    match action.as_str() {
                        "start" => {
                            start_daemon();
                        }
                        "stop" => {
                            stop_daemon();
                        }
                        "restart" | "toggle" => {
                            if is_daemon_running() {
                                stop_daemon();
                                std::thread::sleep(std::time::Duration::from_millis(300));
                                start_daemon();
                            } else {
                                start_daemon();
                            }
                        }
                        _ => {}
                    }
                }
            }
            _ => {}
        }
    }
}

fn is_daemon_running() -> bool {
    unsafe {
        let class_name = encode_wide(WINSPACES_MSG_WINDOW_CLASS);
        let title_name = encode_wide(WINSPACES_MSG_WINDOW_TITLE);
        let hwnd = FindWindowW(class_name.as_ptr(), title_name.as_ptr());
        !hwnd.is_null()
    }
}

fn notify_daemon_reload() {
    unsafe {
        let class_name = encode_wide(WINSPACES_MSG_WINDOW_CLASS);
        let title_name = encode_wide(WINSPACES_MSG_WINDOW_TITLE);
        let hwnd = FindWindowW(class_name.as_ptr(), title_name.as_ptr());
        if !hwnd.is_null() {
            PostMessageW(hwnd, WM_WINSPACES_RELOAD_CONFIG, 0, 0);
        }
    }
}

fn start_daemon() {
    if !is_daemon_running() {
        let daemon_path = std::env::current_exe().ok().and_then(|mut p| {
            p.pop();
            let d = p.join(WINSPACES_DAEMON_EXE);
            if d.exists() {
                Some(d)
            } else {
                None
            }
        });

        let target = daemon_path
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_else(|| WINSPACES_DAEMON_EXE.to_string());

        let _ = std::process::Command::new(target).spawn();
    }
}

fn stop_daemon() {
    unsafe {
        let class_name = encode_wide(WINSPACES_MSG_WINDOW_CLASS);
        let title_name = encode_wide(WINSPACES_MSG_WINDOW_TITLE);
        let hwnd = FindWindowW(class_name.as_ptr(), title_name.as_ptr());
        if !hwnd.is_null() {
            PostMessageW(hwnd, WM_CLOSE, 0, 0);
        }
    }
}

fn get_autostart_status() -> bool {
    unsafe {
        let subkey = encode_wide(REG_RUN_PATH);
        let mut hkey = null_mut();
        if RegOpenKeyExW(HKEY_CURRENT_USER, subkey.as_ptr(), 0, KEY_READ, &mut hkey) == 0 {
            let val_name = encode_wide(REG_APP_NAME);
            let mut type_reg = 0u32;
            let mut data_len = 0u32;
            let res = RegQueryValueExW(
                hkey,
                val_name.as_ptr(),
                null_mut(),
                &mut type_reg,
                null_mut(),
                &mut data_len,
            );
            RegCloseKey(hkey);
            res == 0
        } else {
            false
        }
    }
}

fn set_autostart_status(enable: bool) {
    unsafe {
        let subkey = encode_wide(REG_RUN_PATH);
        let mut hkey = null_mut();
        if RegOpenKeyExW(HKEY_CURRENT_USER, subkey.as_ptr(), 0, KEY_WRITE, &mut hkey) == 0 {
            let val_name = encode_wide(REG_APP_NAME);
            if enable {
                if let Ok(exe) = std::env::current_exe() {
                    let mut daemon_exe = exe.clone();
                    daemon_exe.pop();
                    let daemon_path = daemon_exe.join(WINSPACES_DAEMON_EXE);
                    let wide_path = encode_wide(&daemon_path.to_string_lossy());
                    let _ = RegSetValueExW(
                        hkey,
                        val_name.as_ptr(),
                        0,
                        REG_SZ,
                        wide_path.as_ptr() as _,
                        (wide_path.len() * 2) as u32,
                    );
                }
            } else {
                let _ = RegDeleteValueW(hkey, val_name.as_ptr());
            }
            RegCloseKey(hkey);
        }
    }
}

fn encode_wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}
