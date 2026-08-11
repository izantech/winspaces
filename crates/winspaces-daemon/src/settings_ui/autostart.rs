//! HKCU `Run` autostart entry for the daemon: same key, value name, and
//! quoted-path format the installer writes, so the settings toggle and the
//! installer's autostart option stay in sync.

use windows_sys::Win32::System::Registry::{
    RegCloseKey, RegCreateKeyExW, RegDeleteValueW, RegOpenKeyExW, RegQueryValueExW, RegSetValueExW,
    HKEY, HKEY_CURRENT_USER, KEY_QUERY_VALUE, KEY_SET_VALUE, REG_OPTION_NON_VOLATILE, REG_SZ,
};

use crate::tray::encode_wide;

const RUN_KEY: &str = "Software\\Microsoft\\Windows\\CurrentVersion\\Run";
const APP_NAME: &str = "WinSpaces";

pub fn is_enabled() -> bool {
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

pub fn set_enabled(enable: bool) -> bool {
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
