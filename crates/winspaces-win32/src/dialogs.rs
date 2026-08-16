//! Safe Win32 Open and Save file dialog helpers built on raw Win32 FFI.

use std::path::PathBuf;
use windows_sys::Win32::Foundation::HWND;
use windows_sys::Win32::UI::Controls::Dialogs::{
    GetOpenFileNameW, GetSaveFileNameW, OFN_EXPLORER, OFN_FILEMUSTEXIST, OFN_HIDEREADONLY,
    OFN_NOCHANGEDIR, OFN_OVERWRITEPROMPT, OFN_PATHMUSTEXIST, OPENFILENAMEW,
};

/// Open a native Win32 File Open dialog modal to `owner`.
///
/// `filter` must be formatted as alternating pairs of display strings and patterns,
/// ending with double null (e.g. `"JSON Files (*.json)\0*.json\0All Files (*.*)\0*.*\0\0"`).
pub fn open_file_dialog(owner: HWND, filter: &str, title: &str) -> Option<PathBuf> {
    let mut file_buf = [0u16; 1024];
    let filter_wide: Vec<u16> = filter.encode_utf16().collect();
    let title_wide: Vec<u16> = title.encode_utf16().chain(std::iter::once(0)).collect();

    let mut ofn: OPENFILENAMEW = unsafe { std::mem::zeroed() };
    ofn.lStructSize = std::mem::size_of::<OPENFILENAMEW>() as u32;
    ofn.hwndOwner = owner;
    ofn.lpstrFilter = filter_wide.as_ptr();
    ofn.lpstrFile = file_buf.as_mut_ptr();
    ofn.nMaxFile = file_buf.len() as u32;
    ofn.lpstrTitle = title_wide.as_ptr();
    ofn.Flags =
        OFN_PATHMUSTEXIST | OFN_FILEMUSTEXIST | OFN_HIDEREADONLY | OFN_EXPLORER | OFN_NOCHANGEDIR;

    let res = unsafe { GetOpenFileNameW(&mut ofn) };
    if res != 0 {
        let len = file_buf
            .iter()
            .position(|&c| c == 0)
            .unwrap_or(file_buf.len());
        let path_str = String::from_utf16_lossy(&file_buf[..len]);
        Some(PathBuf::from(path_str))
    } else {
        None
    }
}

/// Open a native Win32 File Save dialog modal to `owner`.
///
/// `default_name` provides the initial suggested filename.
/// `filter` must be formatted as alternating pairs of display strings and patterns,
/// ending with double null (e.g. `"JSON Files (*.json)\0*.json\0All Files (*.*)\0*.*\0\0"`).
pub fn save_file_dialog(
    owner: HWND,
    default_name: &str,
    filter: &str,
    title: &str,
) -> Option<PathBuf> {
    let mut file_buf = [0u16; 1024];
    let name_wide: Vec<u16> = default_name.encode_utf16().collect();
    let copy_len = name_wide.len().min(file_buf.len() - 1);
    file_buf[..copy_len].copy_from_slice(&name_wide[..copy_len]);

    let filter_wide: Vec<u16> = filter.encode_utf16().collect();
    let title_wide: Vec<u16> = title.encode_utf16().chain(std::iter::once(0)).collect();
    let def_ext_wide: Vec<u16> = "json\0".encode_utf16().collect();

    let mut ofn: OPENFILENAMEW = unsafe { std::mem::zeroed() };
    ofn.lStructSize = std::mem::size_of::<OPENFILENAMEW>() as u32;
    ofn.hwndOwner = owner;
    ofn.lpstrFilter = filter_wide.as_ptr();
    ofn.lpstrFile = file_buf.as_mut_ptr();
    ofn.nMaxFile = file_buf.len() as u32;
    ofn.lpstrTitle = title_wide.as_ptr();
    ofn.lpstrDefExt = def_ext_wide.as_ptr();
    // NOCHANGEDIR: without it a save dialog leaves the process sitting in
    // whatever directory the user browsed to, which can pin a removable drive
    // for the lifetime of the settings window.
    ofn.Flags =
        OFN_PATHMUSTEXIST | OFN_OVERWRITEPROMPT | OFN_HIDEREADONLY | OFN_EXPLORER | OFN_NOCHANGEDIR;

    let res = unsafe { GetSaveFileNameW(&mut ofn) };
    if res != 0 {
        let len = file_buf
            .iter()
            .position(|&c| c == 0)
            .unwrap_or(file_buf.len());
        let path_str = String::from_utf16_lossy(&file_buf[..len]);
        Some(PathBuf::from(path_str))
    } else {
        None
    }
}
