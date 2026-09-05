//! Locating the running daemon from any process: the settings window, a
//! control-flag invocation of the exe, or a second daemon checking that it
//! is alone. The daemon is found by its message window's class and title.

use windows_sys::Win32::Foundation::HWND;
use windows_sys::Win32::UI::WindowsAndMessaging::FindWindowW;
use winspaces_common::{WINSPACES_MSG_WINDOW_CLASS, WINSPACES_MSG_WINDOW_TITLE};
use winspaces_win32::text::encode_wide;

/// The daemon's message window, or null when no daemon is running.
pub fn find_daemon_window() -> HWND {
    let class_name = encode_wide(WINSPACES_MSG_WINDOW_CLASS);
    let title = encode_wide(WINSPACES_MSG_WINDOW_TITLE);
    unsafe { FindWindowW(class_name.as_ptr(), title.as_ptr()) }
}

pub fn is_daemon_running() -> bool {
    !find_daemon_window().is_null()
}
