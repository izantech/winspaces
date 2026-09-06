//! Drives the daemon by **posting messages only** — never `SendInput`,
//! never `SetForegroundWindow`, never moving or resizing a foreign window.
//! Every function here either posts to the message window the same way
//! `winspaces.exe --exit` / `--mission-control` / `--tiling-toggle` do, or
//! reproduces the tray's own `WM_TRAYICON` / menu-close sequence.

use std::ptr::null;
use std::time::{Duration, Instant};

use windows_sys::Win32::Foundation::HWND;
use windows_sys::Win32::Graphics::Gdi::{InvalidateRect, UpdateWindow};
use windows_sys::Win32::System::Threading::{
    OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    FindWindowW, GetWindowThreadProcessId, IsWindow, IsWindowVisible, PostMessageW, WM_COMMAND,
    WM_RBUTTONUP,
};
use winspaces_common::{
    WM_WINSPACES_RELOAD_CONFIG, WM_WINSPACES_TILING_TOGGLE, WM_WINSPACES_TOGGLE_MISSION_CONTROL,
};
use winspaces_core::daemon::find_daemon_window;
use winspaces_win32::text::encode_wide;

/// `WM_USER + 1` (`winspaces_ui::tray::WM_TRAYICON`), reproduced here rather
/// than imported because posting it is exactly the "click the tray icon"
/// gesture and belongs next to the hazard it is paired with below.
const WM_TRAYICON: u32 = 0x0400 + 1;

/// **Never post `WM_CLOSE` (0x0010) to the tray menu window.**
/// `DefWindowProc` turns an unhandled `WM_CLOSE` into `DestroyWindow`, so the
/// window dies without `close_menu()` ever running — the only code that
/// unhooks the daemon's global `WH_MOUSE_LL` hook and clears `MENU_STATE`.
/// The result is two stranded global input hooks: `WH_KEYBOARD_LL` keeps
/// swallowing Space/arrows/Enter/Esc system-wide, and the orphaned mouse
/// hook eats button-downs, for as long as the daemon runs. `IsWindow`
/// returns false afterwards, so it looks like it worked. Always post this
/// instead (`WM_APP + 41`, `docs/benchmarks.md` §3).
const WM_MENU_CLOSE: u32 = 0x8000 + 41;

/// `ID_TRAY_SWITCH_BASE` (`crates/winspaces/src/handlers/commands.rs`):
/// `WM_COMMAND` wparam `2000 + mon_idx*100 + space_idx` switches monitor
/// `mon_idx` to space `space_idx` (both 0-based).
const TRAY_SWITCH_BASE: u32 = 2000;

const MENU_CLASS_NAME: &str = "WinSpacesMenu";
const MC_CLASS_NAME: &str = "WinSpacesMissionControl";

const POLL_INTERVAL: Duration = Duration::from_millis(20);

/// Finds the daemon's hidden message window. `Err` means no daemon is
/// running (or `--own` has not started one yet).
pub fn find_message_window() -> Result<HWND, String> {
    let hwnd = find_daemon_window();
    if hwnd.is_null() {
        Err("no running daemon found (FindWindowW returned null)".to_string())
    } else {
        Ok(hwnd)
    }
}

pub fn pid_of(hwnd: HWND) -> u32 {
    let mut pid = 0u32;
    unsafe {
        GetWindowThreadProcessId(hwnd, &mut pid);
    }
    pid
}

/// The daemon's own exe path, read from its process (may fail across
/// integrity levels: `QueryFullProcessImageNameW` needs at least
/// `PROCESS_QUERY_LIMITED_INFORMATION`, which we already hold via the
/// caller's handle, but a *non-elevated* harness cannot always read a path
/// out of an *elevated* process depending on ACLs on the image itself).
pub fn exe_path_of(pid: u32) -> Option<String> {
    unsafe {
        let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if handle.is_null() {
            return None;
        }
        let mut buf = vec![0u16; 1024];
        let mut size = buf.len() as u32;
        let ok =
            QueryFullProcessImageNameW(handle, PROCESS_NAME_WIN32, buf.as_mut_ptr(), &mut size);
        windows_sys::Win32::Foundation::CloseHandle(handle);
        if ok == 0 {
            return None;
        }
        Some(String::from_utf16_lossy(&buf[..size as usize]))
    }
}

/// Switch monitor `mon_idx` (0-based) to space `space_idx` (0-based).
pub fn switch(msgwnd: HWND, mon_idx: usize, space_idx: usize) {
    let id = TRAY_SWITCH_BASE as usize + mon_idx * 100 + space_idx;
    unsafe {
        PostMessageW(msgwnd, WM_COMMAND, id, 0);
    }
}

pub fn toggle_mission_control(msgwnd: HWND) {
    unsafe {
        PostMessageW(msgwnd, WM_WINSPACES_TOGGLE_MISSION_CONTROL, 0, 0);
    }
}

/// Mission Control's overlay window, or null when it has never been shown
/// (it is retained hidden after the first close — by design, not a leak;
/// see `docs/benchmarks.md` §2). `FindWindowW` needs a null title pointer
/// here, not an empty string: the window is titleless.
pub fn mc_window() -> HWND {
    let class = encode_wide(MC_CLASS_NAME);
    unsafe { FindWindowW(class.as_ptr(), null()) }
}

pub fn mc_visible() -> bool {
    let hwnd = mc_window();
    !hwnd.is_null() && unsafe { IsWindowVisible(hwnd) != 0 }
}

/// Waits up to `timeout` for `mc_visible()` to equal `want`, polling every
/// `POLL_INTERVAL`. `false` means the expected effect never appeared —
/// callers must treat that as a reason to skip, not as a zero measurement.
pub fn wait_for_mc_visible(want: bool, timeout: Duration) -> bool {
    wait_until(timeout, || mc_visible() == want)
}

/// Opens the tray context menu exactly as a real right-click would:
/// `WM_TRAYICON` with wparam `1` (icon id) and lparam `WM_RBUTTONUP`,
/// matching `on_tray_icon`'s dispatch in `crates/winspaces/src/handlers/commands.rs`.
pub fn open_menu(msgwnd: HWND) {
    unsafe {
        PostMessageW(msgwnd, WM_TRAYICON, 1, WM_RBUTTONUP as isize);
    }
}

/// Waits up to 2 s for `WinSpacesMenu` to appear, polling every 20 ms.
pub fn menu_window(timeout: Duration) -> Option<HWND> {
    let class = encode_wide(MENU_CLASS_NAME);
    let deadline = Instant::now() + timeout;
    loop {
        let hwnd = unsafe { FindWindowW(class.as_ptr(), null()) };
        if !hwnd.is_null() {
            return Some(hwnd);
        }
        if Instant::now() >= deadline {
            return None;
        }
        std::thread::sleep(POLL_INTERVAL);
    }
}

/// Closes the tray menu the only safe way: see the hazard comment on
/// [`WM_MENU_CLOSE`]. Waits until the window is gone (destroyed) or at
/// least no longer visible, up to `timeout`.
pub fn close_menu(hwnd: HWND, timeout: Duration) -> bool {
    unsafe {
        PostMessageW(hwnd, WM_MENU_CLOSE, 0, 0);
    }
    wait_until(timeout, || unsafe {
        IsWindow(hwnd) == 0 || IsWindowVisible(hwnd) == 0
    })
}

pub fn reload_config(msgwnd: HWND) {
    unsafe {
        PostMessageW(msgwnd, WM_WINSPACES_RELOAD_CONFIG, 0, 0);
    }
}

pub fn toggle_tiling(msgwnd: HWND) {
    unsafe {
        PostMessageW(msgwnd, WM_WINSPACES_TILING_TOGGLE, 0, 0);
    }
}

/// A forced, synchronous repaint: `InvalidateRect` + `UpdateWindow`, so the
/// daemon paints on the caller's `PostMessageW` before this returns.
pub fn force_repaint(hwnd: HWND) {
    unsafe {
        InvalidateRect(hwnd, null(), 0);
        UpdateWindow(hwnd);
    }
}

/// An asynchronous invalidation: `InvalidateRect` alone, so the daemon
/// paints on its own thread whenever it next pumps `WM_PAINT`. Used to
/// drive the in-paint peak (`docs/benchmarks.md` §4.3): the paint DIB only
/// exists inside `WM_PAINT`, so a resting sample never sees it.
pub fn async_invalidate(hwnd: HWND) {
    unsafe {
        InvalidateRect(hwnd, null(), 0);
    }
}

pub fn wait_until<F: FnMut() -> bool>(timeout: Duration, mut f: F) -> bool {
    let deadline = Instant::now() + timeout;
    loop {
        if f() {
            return true;
        }
        if Instant::now() >= deadline {
            return f();
        }
        std::thread::sleep(POLL_INTERVAL);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn switch_command_id_matches_the_daemon_stride() {
        // Mirrors ID_TRAY_SWITCH_BASE + mon_idx*100 + space_idx from
        // crates/winspaces/src/handlers/commands.rs, without linking that
        // crate (the bin sits above winspaces-bench in the dependency order).
        let mon_idx = 2usize;
        let space_idx = 3usize;
        assert_eq!(TRAY_SWITCH_BASE as usize + mon_idx * 100 + space_idx, 2203);
    }

    #[test]
    fn wait_until_returns_true_as_soon_as_the_predicate_is() {
        let mut calls = 0;
        let ok = wait_until(Duration::from_millis(200), || {
            calls += 1;
            calls >= 2
        });
        assert!(ok);
        assert_eq!(calls, 2);
    }

    #[test]
    fn wait_until_times_out_and_returns_the_final_check() {
        let ok = wait_until(Duration::from_millis(50), || false);
        assert!(!ok);
    }
}
