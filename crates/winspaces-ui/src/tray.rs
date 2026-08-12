mod badge;

use badge::create_fluent_badge_icon;
use std::ptr::null_mut;
use windows_sys::Win32::Foundation::HWND;
use windows_sys::Win32::UI::Shell::{
    Shell_NotifyIconW, NIM_ADD, NIM_DELETE, NIM_MODIFY, NOTIFYICONDATAW,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{DestroyIcon, HICON};
use winspaces_common::log_info;
use winspaces_win32::text::encode_wide;

pub const WM_TRAYICON: u32 = windows_sys::Win32::UI::WindowsAndMessaging::WM_USER + 1;
const TRAY_ICON_ID: u32 = 1;

pub struct TrayIcon {
    hwnd: HWND,
    current_icon: HICON,
    /// Badge text currently on screen. `update` is called on every switch,
    /// activation and reconcile; when the text is unchanged the whole
    /// rebuild + `Shell_NotifyIconW` round trip into explorer is skipped.
    last_text: String,
}

impl TrayIcon {
    pub fn new(hwnd: HWND) -> Self {
        let mut s = Self {
            hwnd,
            current_icon: null_mut(),
            last_text: String::from("1"),
        };

        let icon = create_fluent_badge_icon("1");
        s.current_icon = icon;

        let mut nid: NOTIFYICONDATAW = unsafe { std::mem::zeroed() };
        nid.cbSize = std::mem::size_of::<NOTIFYICONDATAW>() as u32;
        nid.hWnd = hwnd;
        nid.uID = TRAY_ICON_ID;
        nid.uFlags = windows_sys::Win32::UI::Shell::NIF_ICON
            | windows_sys::Win32::UI::Shell::NIF_MESSAGE
            | windows_sys::Win32::UI::Shell::NIF_TIP;
        nid.uCallbackMessage = WM_TRAYICON;
        nid.hIcon = icon;

        let tip = encode_wide("WinSpaces");
        let len = tip.len().min(nid.szTip.len());
        nid.szTip[..len].copy_from_slice(&tip[..len]);

        unsafe {
            Shell_NotifyIconW(NIM_ADD, &nid);
        }

        s
    }

    pub fn update(&mut self, text: &str) {
        if text == self.last_text && !self.current_icon.is_null() {
            return;
        }

        let new_icon = create_fluent_badge_icon(text);

        let mut nid: NOTIFYICONDATAW = unsafe { std::mem::zeroed() };
        nid.cbSize = std::mem::size_of::<NOTIFYICONDATAW>() as u32;
        nid.hWnd = self.hwnd;
        nid.uID = TRAY_ICON_ID;
        nid.uFlags =
            windows_sys::Win32::UI::Shell::NIF_ICON | windows_sys::Win32::UI::Shell::NIF_TIP;
        nid.hIcon = new_icon;

        let tip = encode_wide(&format!("WinSpaces ({})", text));
        let len = tip.len().min(nid.szTip.len());
        nid.szTip[..len].copy_from_slice(&tip[..len]);

        unsafe {
            Shell_NotifyIconW(NIM_MODIFY, &nid);
            if !self.current_icon.is_null() {
                DestroyIcon(self.current_icon);
            }
        }
        self.current_icon = new_icon;
        self.last_text = text.to_string();
    }

    pub fn remove(&mut self) {
        if !self.current_icon.is_null() {
            log_info!("Removing tray icon...");
            let mut nid: NOTIFYICONDATAW = unsafe { std::mem::zeroed() };
            nid.cbSize = std::mem::size_of::<NOTIFYICONDATAW>() as u32;
            nid.hWnd = self.hwnd;
            nid.uID = TRAY_ICON_ID;
            unsafe {
                Shell_NotifyIconW(NIM_DELETE, &nid);
                DestroyIcon(self.current_icon);
            }
            self.current_icon = null_mut();
            self.last_text.clear();
        }
    }
}

impl Drop for TrayIcon {
    fn drop(&mut self) {
        self.remove();
    }
}
