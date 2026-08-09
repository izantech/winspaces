use std::ptr::null_mut;
use windows_sys::Win32::Foundation::{HWND, SIZE};
use windows_sys::Win32::Graphics::Gdi::{
    CreateCompatibleBitmap, CreateCompatibleDC, CreateFontW, DeleteDC, DeleteObject, GetDC,
    GetDeviceCaps, GetTextExtentPoint32W, PatBlt, ReleaseDC, SelectObject, SetBkColor,
    SetTextColor, TextOutW, BLACKNESS, LOGPIXELSY,
};
use windows_sys::Win32::UI::Shell::{
    Shell_NotifyIconW, NIF_ICON, NIF_MESSAGE, NIF_TIP, NIM_ADD, NIM_DELETE, NIM_MODIFY,
    NOTIFYICONDATAW,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CreateIconIndirect, DestroyIcon, GetSystemMetrics, HICON, ICONINFO, SM_CXSMICON, WM_APP,
};

pub const WM_TRAYICON: u32 = WM_APP + 1;

pub struct TrayIcon {
    pub hwnd: HWND,
    current_icon: Option<HICON>,
}

impl TrayIcon {
    pub fn new(hwnd: HWND) -> Self {
        let mut tray = Self {
            hwnd,
            current_icon: None,
        };
        tray.init();
        tray
    }

    pub fn init(&mut self) {
        let mut nid = unsafe { std::mem::zeroed::<NOTIFYICONDATAW>() };
        nid.cbSize = std::mem::size_of::<NOTIFYICONDATAW>() as u32;
        nid.hWnd = self.hwnd;
        nid.uID = 1;
        nid.uFlags = NIF_MESSAGE | NIF_TIP;
        nid.uCallbackMessage = WM_TRAYICON;

        let tip = encode_wide("WinSpaces");
        let len = tip.len().min(nid.szTip.len() - 1);
        nid.szTip[..len].copy_from_slice(&tip[..len]);

        unsafe {
            Shell_NotifyIconW(NIM_ADD, &nid);
        }
    }

    pub fn update(&mut self, label: &str) {
        let hicon = create_text_icon(label);

        if let Some(old_icon) = self.current_icon.take() {
            unsafe {
                DestroyIcon(old_icon);
            }
        }
        self.current_icon = Some(hicon);

        let mut nid = unsafe { std::mem::zeroed::<NOTIFYICONDATAW>() };
        nid.cbSize = std::mem::size_of::<NOTIFYICONDATAW>() as u32;
        nid.hWnd = self.hwnd;
        nid.uID = 1;
        nid.uFlags = NIF_ICON | NIF_MESSAGE | NIF_TIP;
        nid.uCallbackMessage = WM_TRAYICON;
        nid.hIcon = hicon;

        let tip = encode_wide(&format!("WinSpaces - Desktop {}", label));
        let len = tip.len().min(nid.szTip.len() - 1);
        nid.szTip[..len].copy_from_slice(&tip[..len]);

        unsafe {
            Shell_NotifyIconW(NIM_MODIFY, &nid);
        }
    }

    pub fn remove(&mut self) {
        let mut nid = unsafe { std::mem::zeroed::<NOTIFYICONDATAW>() };
        nid.cbSize = std::mem::size_of::<NOTIFYICONDATAW>() as u32;
        nid.hWnd = self.hwnd;
        nid.uID = 1;

        unsafe {
            Shell_NotifyIconW(NIM_DELETE, &nid);
            if let Some(icon) = self.current_icon.take() {
                DestroyIcon(icon);
            }
        }
    }
}

/// Render the tray bitmap like the reference C implementation:
/// solid black background, bright green text, centered, Arial.
fn create_text_icon(text: &str) -> HICON {
    unsafe {
        let size = GetSystemMetrics(SM_CXSMICON).max(16);

        let hdc_screen = GetDC(null_mut());
        let hdc_mem = CreateCompatibleDC(hdc_screen);
        let hbmp = CreateCompatibleBitmap(hdc_screen, size, size);
        let old_bmp = SelectObject(hdc_mem, hbmp as _);

        PatBlt(hdc_mem, 0, 0, size, size, BLACKNESS);

        SetBkColor(hdc_mem, 0x00000000);
        SetTextColor(hdc_mem, 0x0000FF00);

        let logpix = GetDeviceCaps(hdc_mem, LOGPIXELSY as i32);
        let font_height = -((11 * logpix) / 72);
        let font_name = encode_wide("Arial");
        let hfont = CreateFontW(
            font_height,
            0,
            0,
            0,
            400, // FW_NORMAL
            0,
            0,
            0,
            0,
            0,
            0,
            0,
            0,
            font_name.as_ptr(),
        );
        let old_font = SelectObject(hdc_mem, hfont as _);

        let text_wide = encode_wide(text);
        let chars = text_wide.len().saturating_sub(1); // drop trailing NUL
        let mut extent = SIZE { cx: 0, cy: 0 };
        GetTextExtentPoint32W(hdc_mem, text_wide.as_ptr(), chars as i32, &mut extent);
        let mut x = (size - extent.cx) / 2;
        if x < 0 {
            x = 0;
        }
        let mut y = (size - extent.cy) / 2;
        if y < 0 {
            y = 0;
        }
        TextOutW(hdc_mem, x, y, text_wide.as_ptr(), chars as i32);

        SelectObject(hdc_mem, old_font);
        DeleteObject(hfont as _);
        SelectObject(hdc_mem, old_bmp);
        DeleteDC(hdc_mem);
        ReleaseDC(null_mut(), hdc_screen);

        let mut icon_info = ICONINFO {
            fIcon: 1,
            xHotspot: 0,
            yHotspot: 0,
            hbmMask: hbmp,
            hbmColor: hbmp,
        };

        let hicon = CreateIconIndirect(&mut icon_info);
        DeleteObject(hbmp as _);
        hicon
    }
}

pub fn encode_wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}
