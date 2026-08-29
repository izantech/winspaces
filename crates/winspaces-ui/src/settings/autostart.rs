//! Autostart and elevated execution management for the daemon.
//! Supports both the classic HKCU `Run` registry key (for non-elevated mode)
//! and Windows Task Scheduler highest-privilege task (for elevated mode).

use std::os::windows::process::CommandExt;
use std::process::Command;
use windows_sys::Win32::Foundation::{CloseHandle, GetLastError};
use windows_sys::Win32::System::Registry::{
    RegCloseKey, RegCreateKeyExW, RegDeleteValueW, RegOpenKeyExW, RegQueryValueExW, RegSetValueExW,
    HKEY, HKEY_CURRENT_USER, KEY_QUERY_VALUE, KEY_SET_VALUE, REG_OPTION_NON_VOLATILE, REG_SZ,
};
use windows_sys::Win32::System::Threading::{GetExitCodeProcess, WaitForSingleObject, INFINITE};
use windows_sys::Win32::UI::Shell::{ShellExecuteExW, SEE_MASK_NOCLOSEPROCESS, SHELLEXECUTEINFOW};
use windows_sys::Win32::UI::WindowsAndMessaging::{FindWindowW, GetPropW, SW_HIDE};
use winspaces_common::{WINSPACES_MSG_WINDOW_CLASS, WINSPACES_MSG_WINDOW_TITLE};
use winspaces_win32::text::encode_wide;

const RUN_KEY: &str = "Software\\Microsoft\\Windows\\CurrentVersion\\Run";
const APP_NAME: &str = "WinSpaces";
pub const ELEVATED_TASK_NAME: &str = "WinSpaces Daemon (Elevated)";
pub const PROP_ELEVATED: &str = "WinSpacesElevated";

const CREATE_NO_WINDOW: u32 = 0x08000000;

/// Check if the classic HKCU `Run` registry key is present.
pub fn is_run_key_enabled() -> bool {
    unsafe {
        let mut key: HKEY = std::ptr::null_mut();
        let path = encode_wide(RUN_KEY);
        if RegOpenKeyExW(
            HKEY_CURRENT_USER,
            path.as_ptr(),
            0,
            KEY_QUERY_VALUE,
            &mut key,
        ) != 0
        {
            return false;
        }
        let value_name = encode_wide(APP_NAME);
        let mut len = 0u32;
        let found = RegQueryValueExW(
            key,
            value_name.as_ptr(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            &mut len,
        ) == 0;
        RegCloseKey(key);
        found
    }
}

/// Set or remove the classic HKCU `Run` registry key.
pub fn set_run_key_enabled(enable: bool) -> bool {
    unsafe {
        let mut key: HKEY = std::ptr::null_mut();
        let path = encode_wide(RUN_KEY);
        if RegCreateKeyExW(
            HKEY_CURRENT_USER,
            path.as_ptr(),
            0,
            std::ptr::null(),
            REG_OPTION_NON_VOLATILE,
            KEY_QUERY_VALUE | KEY_SET_VALUE,
            std::ptr::null(),
            &mut key,
            std::ptr::null_mut(),
        ) != 0
        {
            return false;
        }
        let value_name = encode_wide(APP_NAME);
        let ok = if enable {
            let exe = match std::env::current_exe() {
                Ok(p) => p,
                Err(_) => {
                    RegCloseKey(key);
                    return false;
                }
            };
            let data = encode_wide(&format!("\"{}\"", exe.display()));
            RegSetValueExW(
                key,
                value_name.as_ptr(),
                0,
                REG_SZ,
                data.as_ptr() as _,
                (data.len() * 2) as u32,
            ) == 0
        } else {
            const ERROR_FILE_NOT_FOUND: u32 = 2;
            let rc = RegDeleteValueW(key, value_name.as_ptr());
            rc == 0 || rc == ERROR_FILE_NOT_FOUND
        };
        RegCloseKey(key);
        ok
    }
}

/// Query whether the elevated scheduled task is registered.
pub fn is_elevated_task_installed() -> bool {
    let output = Command::new("schtasks.exe")
        .args(["/query", "/tn", ELEVATED_TASK_NAME])
        .creation_flags(CREATE_NO_WINDOW)
        .output();
    match output {
        Ok(out) => out.status.success(),
        Err(_) => false,
    }
}

/// Check whether the currently running daemon is elevated.
pub fn is_daemon_elevated() -> bool {
    unsafe {
        let class_name = encode_wide(WINSPACES_MSG_WINDOW_CLASS);
        let title = encode_wide(WINSPACES_MSG_WINDOW_TITLE);
        let hwnd = FindWindowW(class_name.as_ptr(), title.as_ptr());
        if hwnd.is_null() {
            return false;
        }
        let prop = encode_wide(PROP_ELEVATED);
        !GetPropW(hwnd, prop.as_ptr()).is_null()
    }
}

/// Check if elevated mode is active (scheduled task registered or live daemon elevated).
pub fn is_elevated_mode_active() -> bool {
    is_elevated_task_installed() || is_daemon_elevated()
}

/// Overall autostart state: enabled if either the elevated task or the HKCU Run key is set.
pub fn is_autostart_enabled() -> bool {
    is_elevated_task_installed() || is_run_key_enabled()
}

#[derive(Debug, PartialEq, Eq)]
pub enum ElevationResult {
    Success,
    Cancelled,
    Failed(String),
}

/// Request UAC elevation once to run an internal winspaces command flag (e.g. `--enable-elevation`).
pub fn run_elevated_command(flag: &str) -> ElevationResult {
    let exe = match std::env::current_exe() {
        Ok(p) => p,
        Err(e) => return ElevationResult::Failed(format!("Could not get current executable: {e}")),
    };

    let exe_wide = encode_wide(&exe.to_string_lossy());
    let verb_wide = encode_wide("runas");
    let params_wide = encode_wide(flag);

    let mut info = SHELLEXECUTEINFOW {
        cbSize: std::mem::size_of::<SHELLEXECUTEINFOW>() as u32,
        fMask: SEE_MASK_NOCLOSEPROCESS,
        hwnd: std::ptr::null_mut(),
        lpVerb: verb_wide.as_ptr(),
        lpFile: exe_wide.as_ptr(),
        lpParameters: params_wide.as_ptr(),
        lpDirectory: std::ptr::null(),
        nShow: SW_HIDE as _,
        hInstApp: std::ptr::null_mut(),
        lpIDList: std::ptr::null_mut(),
        lpClass: std::ptr::null(),
        hkeyClass: std::ptr::null_mut(),
        dwHotKey: 0,
        Anonymous: unsafe { std::mem::zeroed() },
        hProcess: std::ptr::null_mut(),
    };

    unsafe {
        if ShellExecuteExW(&mut info) == 0 {
            let err = GetLastError();
            const ERROR_CANCELLED: u32 = 1223;
            if err == ERROR_CANCELLED {
                return ElevationResult::Cancelled;
            }
            return ElevationResult::Failed(format!("ShellExecuteEx failed with error {err}"));
        }

        if !info.hProcess.is_null() {
            WaitForSingleObject(info.hProcess, INFINITE);
            let mut exit_code = 0u32;
            GetExitCodeProcess(info.hProcess, &mut exit_code);
            CloseHandle(info.hProcess);
            if exit_code == 0 {
                ElevationResult::Success
            } else {
                ElevationResult::Failed(format!("Process exited with error code {exit_code}"))
            }
        } else {
            ElevationResult::Success
        }
    }
}
