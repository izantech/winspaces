//! The `--dump` diagnostic: write every top-level window's metrics to a file.

use windows_sys::Win32::Foundation::RECT;

use super::query::{
    get_process_image_path, get_window_class, get_window_placement_info, get_window_title,
};
use crate::desktop::is_valid_window;

/// Write every top-level window's metrics to `out_file`.
///
/// Strictly read-only, and it must stay that way: this runs as a separate
/// process while a daemon may be live. It deliberately takes no
/// `DesktopManager` — constructing one calls `reclaim_orphaned_windows`, which
/// un-cloaks every window the running daemon has hidden on inactive spaces.
/// The parameter used to exist and was never read; the constructor grew that
/// side effect later, silently making a diagnostic destructive.
///
/// # Safety
/// Walks the desktop's top-level window chain via raw `GetWindow` traversal;
/// must be called from a thread that may legally enumerate windows.
pub unsafe fn dump_all_window_metrics(out_file: &str) {
    let mut output = format!(
        "=== WINSPACES RUST WINDOW DUMP ({}) ===\n",
        chrono_format_now()
    );
    let mut hwnd = windows_sys::Win32::UI::WindowsAndMessaging::GetWindow(
        windows_sys::Win32::UI::WindowsAndMessaging::GetDesktopWindow(),
        windows_sys::Win32::UI::WindowsAndMessaging::GW_CHILD,
    );
    while !hwnd.is_null() {
        let title = get_window_title(hwnd);
        let class_name = get_window_class(hwnd);
        let exe_path = get_process_image_path(hwnd);

        if !title.is_empty() || !class_name.is_empty() {
            let mut win_rect: RECT = std::mem::zeroed();
            windows_sys::Win32::UI::WindowsAndMessaging::GetWindowRect(hwnd, &mut win_rect);

            let mut frame_rect: RECT = std::mem::zeroed();
            windows_sys::Win32::Graphics::Dwm::DwmGetWindowAttribute(
                hwnd,
                windows_sys::Win32::Graphics::Dwm::DWMWA_EXTENDED_FRAME_BOUNDS as _,
                &mut frame_rect as *mut _ as _,
                std::mem::size_of::<RECT>() as u32,
            );

            let vis = windows_sys::Win32::UI::WindowsAndMessaging::IsWindowVisible(hwnd);
            let (show_cmd, rect) = get_window_placement_info(hwnd);

            let ex_style = windows_sys::Win32::UI::WindowsAndMessaging::GetWindowLongW(
                hwnd,
                windows_sys::Win32::UI::WindowsAndMessaging::GWL_EXSTYLE,
            ) as u32;
            let owner = windows_sys::Win32::UI::WindowsAndMessaging::GetAncestor(
                hwnd,
                windows_sys::Win32::UI::WindowsAndMessaging::GA_ROOTOWNER,
            );
            let mut cloaked: u32 = 0;
            windows_sys::Win32::Graphics::Dwm::DwmGetWindowAttribute(
                hwnd,
                windows_sys::Win32::Graphics::Dwm::DWMWA_CLOAKED as _,
                &mut cloaked as *mut _ as _,
                std::mem::size_of::<u32>() as u32,
            );
            let valid = is_valid_window(hwnd);

            let w = win_rect.right - win_rect.left;
            let h = win_rect.bottom - win_rect.top;

            let line = format!(
                "==========================================\nHWND: {:?}, Vis={}\nTitle: '{}'\nClass: '{}'\nExe: '{}'\nEligibility: Valid={}, ExStyle=0x{:08X}, RootOwner={:?}, Cloaked={}\nGetWindowRect: Left={}, Top={}, Right={}, Bottom={} [W={}, H={}]\nDwmFrameBounds: Left={}, Top={}, Right={}, Bottom={}\nPlacement: showCmd={}, NormalPos: Left={}, Top={}, Right={}, Bottom={}\n",
                hwnd, vis, title, class_name, exe_path,
                valid, ex_style, owner, cloaked,
                win_rect.left, win_rect.top, win_rect.right, win_rect.bottom, w, h,
                frame_rect.left, frame_rect.top, frame_rect.right, frame_rect.bottom,
                show_cmd, rect.left, rect.top, rect.right, rect.bottom
            );
            output.push_str(&line);
        }
        hwnd = windows_sys::Win32::UI::WindowsAndMessaging::GetWindow(
            hwnd,
            windows_sys::Win32::UI::WindowsAndMessaging::GW_HWNDNEXT,
        );
    }
    let _ = std::fs::write(out_file, output);
}

fn chrono_format_now() -> String {
    format!("{:?}", std::time::SystemTime::now())
}
