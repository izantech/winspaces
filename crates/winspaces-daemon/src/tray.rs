use crate::log_info;
use std::ptr::null_mut;
use windows_sys::Win32::Foundation::HWND;
use windows_sys::Win32::Graphics::Gdi::{
    CreateBitmap, CreateCompatibleDC, CreateDIBSection, CreateFontW, CreateSolidBrush, DeleteDC,
    DeleteObject, DrawTextW, FillRect, FrameRect, GetDC, GetStockObject, ReleaseDC, SelectObject,
    SetBkMode, SetTextColor, BITMAPINFO, BITMAPINFOHEADER, BI_RGB, BLACK_BRUSH, CLEARTYPE_QUALITY,
    CLIP_DEFAULT_PRECIS, DEFAULT_CHARSET, DEFAULT_PITCH, DIB_RGB_COLORS, DT_CENTER, DT_SINGLELINE,
    DT_VCENTER, FW_BOLD, OUT_DEFAULT_PRECIS,
};
use windows_sys::Win32::UI::Shell::{
    Shell_NotifyIconW, NIM_ADD, NIM_DELETE, NIM_MODIFY, NOTIFYICONDATAW,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CreateIconIndirect, DestroyIcon, GetSystemMetrics, HICON, ICONINFO, SM_CXSMICON, SM_CYSMICON,
};

pub const WM_TRAYICON: u32 = windows_sys::Win32::UI::WindowsAndMessaging::WM_USER + 1;
const TRAY_ICON_ID: u32 = 1;

pub struct TrayIcon {
    hwnd: HWND,
    current_icon: HICON,
}

impl TrayIcon {
    pub fn new(hwnd: HWND) -> Self {
        let mut s = Self {
            hwnd,
            current_icon: null_mut(),
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
        }
    }
}

impl Drop for TrayIcon {
    fn drop(&mut self) {
        self.remove();
    }
}

pub fn encode_wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

fn is_point_in_round_rect(x: i32, y: i32, w: i32, h: i32, r: i32) -> bool {
    if x < 0 || y < 0 || x >= w || y >= h {
        return false;
    }
    let cx = if x < r {
        r
    } else if x >= w - r {
        w - r - 1
    } else {
        x
    };
    let cy = if y < r {
        r
    } else if y >= h - r {
        h - r - 1
    } else {
        y
    };
    let dx = x - cx;
    let dy = y - cy;
    (dx * dx + dy * dy) <= (r * r)
}

fn create_fluent_badge_icon(text: &str) -> HICON {
    unsafe {
        let width = GetSystemMetrics(SM_CXSMICON).max(16);
        let height = GetSystemMetrics(SM_CYSMICON).max(16);

        let hdc_screen = GetDC(null_mut());
        let hdc_mem = CreateCompatibleDC(hdc_screen);

        let mut bmi: BITMAPINFO = std::mem::zeroed();
        bmi.bmiHeader.biSize = std::mem::size_of::<BITMAPINFOHEADER>() as u32;
        bmi.bmiHeader.biWidth = width;
        bmi.bmiHeader.biHeight = -height; // Top-down
        bmi.bmiHeader.biPlanes = 1;
        bmi.bmiHeader.biBitCount = 32;
        bmi.bmiHeader.biCompression = BI_RGB;

        let mut bits: *mut u8 = null_mut();
        let hbm_color = CreateDIBSection(
            hdc_screen,
            &bmi,
            DIB_RGB_COLORS,
            &mut bits as *mut *mut u8 as _,
            null_mut(),
            0,
        );

        let old_bm = SelectObject(hdc_mem, hbm_color as _);

        // Clear background with black
        let rect_full = windows_sys::Win32::Foundation::RECT {
            left: 0,
            top: 0,
            right: width,
            bottom: height,
        };
        let brush_black = GetStockObject(BLACK_BRUSH as _);
        FillRect(hdc_mem, &rect_full, brush_black as _);

        // Fill badge background: Windows 11 Dark Slate (#202020 => BGR 0x00202020)
        let bg_brush = CreateSolidBrush(0x00202020);
        FillRect(hdc_mem, &rect_full, bg_brush as _);

        // Draw top accent line: Windows 11 Accent Blue (#0078D4 => BGR 0x00D47800)
        let accent_brush = CreateSolidBrush(0x00D47800);
        let rect_accent = windows_sys::Win32::Foundation::RECT {
            left: 0,
            top: 0,
            right: width,
            bottom: (height / 8).max(2),
        };
        FillRect(hdc_mem, &rect_accent, accent_brush as _);

        // Draw subtle border around badge (#3A3A3A => BGR 0x003A3A3A)
        let border_brush = CreateSolidBrush(0x003A3A3A);
        FrameRect(hdc_mem, &rect_full, border_brush as _);

        DeleteObject(bg_brush as _);
        DeleteObject(accent_brush as _);
        DeleteObject(border_brush as _);

        // Typography: Bold ClearType Segoe UI Variable / Segoe UI
        let font_height = if text.len() > 3 {
            -(height * 50 / 100)
        } else if text.len() > 1 {
            -(height * 60 / 100)
        } else {
            -(height * 75 / 100)
        };

        let font_name = encode_wide("Segoe UI Variable Display");
        let mut hfont = CreateFontW(
            font_height,
            0,
            0,
            0,
            FW_BOLD as i32,
            0,
            0,
            0,
            DEFAULT_CHARSET as u32,
            OUT_DEFAULT_PRECIS as u32,
            CLIP_DEFAULT_PRECIS as u32,
            CLEARTYPE_QUALITY as u32,
            DEFAULT_PITCH as u32,
            font_name.as_ptr(),
        );

        if hfont.is_null() {
            let font_name_fallback = encode_wide("Segoe UI");
            hfont = CreateFontW(
                font_height,
                0,
                0,
                0,
                FW_BOLD as i32,
                0,
                0,
                0,
                DEFAULT_CHARSET as u32,
                OUT_DEFAULT_PRECIS as u32,
                CLIP_DEFAULT_PRECIS as u32,
                CLEARTYPE_QUALITY as u32,
                DEFAULT_PITCH as u32,
                font_name_fallback.as_ptr(),
            );
        }

        let old_font = SelectObject(hdc_mem, hfont as _);
        SetBkMode(
            hdc_mem,
            windows_sys::Win32::Graphics::Gdi::TRANSPARENT as i32,
        );
        SetTextColor(hdc_mem, 0x00FFFFFF);

        let mut rect_text = rect_full;
        rect_text.top += (height / 8).max(1);
        let mut wide_text = encode_wide(text);
        wide_text.pop();

        DrawTextW(
            hdc_mem,
            wide_text.as_ptr(),
            wide_text.len() as i32,
            &mut rect_text,
            DT_CENTER | DT_VCENTER | DT_SINGLELINE,
        );

        SelectObject(hdc_mem, old_font);
        DeleteObject(hfont as _);

        SelectObject(hdc_mem, old_bm);
        DeleteDC(hdc_mem);
        ReleaseDC(null_mut(), hdc_screen);

        // 32-bit ARGB Alpha Mask Fixup (Sub-pixel Alpha Transparency)
        let corner_radius = (width / 4).max(4);
        let pixel_count = (width * height) as usize;
        let slice = std::slice::from_raw_parts_mut(bits as *mut u32, pixel_count);

        for y in 0..height {
            for x in 0..width {
                let idx = (y * width + x) as usize;
                let pixel = slice[idx];
                let b = (pixel & 0xFF) as u8;
                let g = ((pixel >> 8) & 0xFF) as u8;
                let r = ((pixel >> 16) & 0xFF) as u8;

                if is_point_in_round_rect(x, y, width, height, corner_radius) {
                    // Set full opacity (Alpha = 255) for pixels inside rounded squircle badge
                    slice[idx] = (255 << 24) | ((r as u32) << 16) | ((g as u32) << 8) | (b as u32);
                } else {
                    // Transparent alpha = 0 for outer corner pixels
                    slice[idx] = 0;
                }
            }
        }

        // Mask bitmap: 1-bit mask required by CreateIconIndirect
        let hbm_mask = CreateBitmap(width, height, 1, 1, null_mut());

        let ii = ICONINFO {
            fIcon: 1,
            xHotspot: 0,
            yHotspot: 0,
            hbmMask: hbm_mask,
            hbmColor: hbm_color,
        };

        let icon = CreateIconIndirect(&ii);

        DeleteObject(hbm_color as _);
        DeleteObject(hbm_mask as _);

        icon
    }
}
