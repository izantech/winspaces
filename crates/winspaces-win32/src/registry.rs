//! HKCU registry access: the handful of per-user values WinSpaces reads and
//! writes (the settings window's theme preference, the autostart `Run`
//! entry) and the OS's own apps-theme flag. Every function opens and closes
//! its own key; a missing key or value is an ordinary `None`/`false`.

use std::ptr::{null, null_mut};
use windows_sys::Win32::Foundation::ERROR_FILE_NOT_FOUND;
use windows_sys::Win32::System::Registry::{
    RegCloseKey, RegCreateKeyExW, RegDeleteValueW, RegOpenKeyExW, RegQueryValueExW, RegSetValueExW,
    HKEY, HKEY_CURRENT_USER, KEY_QUERY_VALUE, KEY_SET_VALUE, REG_OPTION_NON_VOLATILE, REG_SZ,
};

use crate::text::encode_wide;

/// An open key, closed on drop.
struct Key(HKEY);

impl Drop for Key {
    fn drop(&mut self) {
        unsafe {
            RegCloseKey(self.0);
        }
    }
}

fn open_for_query(subkey: &str) -> Option<Key> {
    let path = encode_wide(subkey);
    let mut key: HKEY = null_mut();
    let rc = unsafe {
        RegOpenKeyExW(
            HKEY_CURRENT_USER,
            path.as_ptr(),
            0,
            KEY_QUERY_VALUE,
            &mut key,
        )
    };
    (rc == 0).then_some(Key(key))
}

/// Creates the key when it does not exist yet.
fn open_for_write(subkey: &str) -> Option<Key> {
    let path = encode_wide(subkey);
    let mut key: HKEY = null_mut();
    let rc = unsafe {
        RegCreateKeyExW(
            HKEY_CURRENT_USER,
            path.as_ptr(),
            0,
            null(),
            REG_OPTION_NON_VOLATILE,
            KEY_QUERY_VALUE | KEY_SET_VALUE,
            null(),
            &mut key,
            null_mut(),
        )
    };
    (rc == 0).then_some(Key(key))
}

/// A `REG_SZ` value under `HKCU\<subkey>`; `None` when the key or value is
/// missing, of another type, or longer than a few hundred characters.
pub fn read_hkcu_string(subkey: &str, value: &str) -> Option<String> {
    let key = open_for_query(subkey)?;
    let name = encode_wide(value);
    let mut buf = [0u16; 512];
    let mut len = (buf.len() * 2) as u32;
    let mut kind = 0u32;
    let rc = unsafe {
        RegQueryValueExW(
            key.0,
            name.as_ptr(),
            null_mut(),
            &mut kind,
            buf.as_mut_ptr() as _,
            &mut len,
        )
    };
    if rc != 0 || kind != REG_SZ {
        return None;
    }
    let chars = (len as usize / 2).min(buf.len());
    Some(
        String::from_utf16_lossy(&buf[..chars])
            .trim_end_matches('\0')
            .to_string(),
    )
}

/// The first four bytes of a value under `HKCU\<subkey>` as a `u32` (a
/// `REG_DWORD` in practice); `None` when the key or value is missing.
pub fn read_hkcu_u32(subkey: &str, value: &str) -> Option<u32> {
    let key = open_for_query(subkey)?;
    let name = encode_wide(value);
    let mut data = 0u32;
    let mut len = 4u32;
    let mut kind = 0u32;
    let rc = unsafe {
        RegQueryValueExW(
            key.0,
            name.as_ptr(),
            null_mut(),
            &mut kind,
            &mut data as *mut u32 as _,
            &mut len,
        )
    };
    (rc == 0).then_some(data)
}

/// Whether `value` exists under `HKCU\<subkey>`, whatever its type.
pub fn hkcu_value_exists(subkey: &str, value: &str) -> bool {
    let Some(key) = open_for_query(subkey) else {
        return false;
    };
    let name = encode_wide(value);
    let mut len = 0u32;
    unsafe {
        RegQueryValueExW(
            key.0,
            name.as_ptr(),
            null_mut(),
            null_mut(),
            null_mut(),
            &mut len,
        ) == 0
    }
}

/// Write `data` as a `REG_SZ` value, creating the key if needed.
pub fn write_hkcu_string(subkey: &str, value: &str, data: &str) -> bool {
    let Some(key) = open_for_write(subkey) else {
        return false;
    };
    let name = encode_wide(value);
    let wide = encode_wide(data);
    unsafe {
        RegSetValueExW(
            key.0,
            name.as_ptr(),
            0,
            REG_SZ,
            wide.as_ptr() as _,
            (wide.len() * 2) as u32,
        ) == 0
    }
}

/// Delete `value`; a value that was already absent counts as success.
pub fn delete_hkcu_value(subkey: &str, value: &str) -> bool {
    let Some(key) = open_for_write(subkey) else {
        return false;
    };
    let name = encode_wide(value);
    let rc = unsafe { RegDeleteValueW(key.0, name.as_ptr()) };
    rc == 0 || rc == ERROR_FILE_NOT_FOUND
}
