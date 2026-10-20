//! Window icon probing shared by the surfaces that label other processes' windows.

use std::ptr::null_mut;
use windows_sys::Win32::Foundation::HWND;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    GCLP_HICON, GCLP_HICONSM, HICON, ICON_BIG, ICON_SMALL, ICON_SMALL2, WM_GETICON,
};

/// `SendMessageW(WM_GETICON)` would block the daemon indefinitely on a hung
/// target; use a short abort-if-hung timeout instead.
unsafe fn send_geticon_timeout(hwnd: HWND, icon_kind: u32) -> HICON {
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        SendMessageTimeoutW, SMTO_ABORTIFHUNG, SMTO_BLOCK,
    };
    let mut result: usize = 0;
    let ok = SendMessageTimeoutW(
        hwnd,
        WM_GETICON,
        icon_kind as _,
        0,
        SMTO_ABORTIFHUNG | SMTO_BLOCK,
        100,
        &mut result,
    );
    if ok != 0 {
        result as HICON
    } else {
        null_mut()
    }
}

/// The window's small icon, falling back to its big icon and then its class icons.
pub(crate) unsafe fn get_window_icon(hwnd: HWND) -> HICON {
    let mut hicon = send_geticon_timeout(hwnd, ICON_SMALL2);
    if hicon.is_null() {
        hicon = send_geticon_timeout(hwnd, ICON_SMALL);
    }
    if hicon.is_null() {
        hicon = send_geticon_timeout(hwnd, ICON_BIG);
    }
    if hicon.is_null() {
        #[cfg(target_pointer_width = "64")]
        {
            hicon =
                windows_sys::Win32::UI::WindowsAndMessaging::GetClassLongPtrW(hwnd, GCLP_HICONSM)
                    as HICON;
        }
        #[cfg(not(target_pointer_width = "64"))]
        {
            hicon = windows_sys::Win32::UI::WindowsAndMessaging::GetClassLongW(hwnd, GCLP_HICONSM)
                as HICON;
        }
    }
    if hicon.is_null() {
        #[cfg(target_pointer_width = "64")]
        {
            hicon = windows_sys::Win32::UI::WindowsAndMessaging::GetClassLongPtrW(hwnd, GCLP_HICON)
                as HICON;
        }
        #[cfg(not(target_pointer_width = "64"))]
        {
            hicon = windows_sys::Win32::UI::WindowsAndMessaging::GetClassLongW(hwnd, GCLP_HICON)
                as HICON;
        }
    }
    hicon
}
