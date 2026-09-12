//! Window-class registration, unifying the menu's `ensure_class`, the
//! settings window's `register_class` and Overview's inline
//! `WNDCLASSEXW` setup — all three built the same shape by hand.

use std::collections::HashSet;
use std::ptr::null_mut;
use std::sync::{Mutex, OnceLock};

use windows_sys::Win32::UI::WindowsAndMessaging::{
    LoadCursorW, LoadIconW, RegisterClassExW, CS_HREDRAW, CS_VREDRAW, IDC_ARROW, WNDCLASSEXW,
    WNDPROC,
};

use crate::module::app_instance;
use crate::text::encode_wide;

fn registered() -> &'static Mutex<HashSet<String>> {
    static REGISTERED: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();
    REGISTERED.get_or_init(|| Mutex::new(HashSet::new()))
}

/// Register a window class with the shape every owner-drawn surface here
/// uses: `CS_HREDRAW | CS_VREDRAW`, the system arrow cursor, a null
/// background brush (every surface paints its own), the application icon
/// (resource 1, embedded by the daemon crate's build script; a null handle
/// in a test binary just means the default icon) and the process module as
/// `hInstance`. Guarded so a given `name` is only ever registered once
/// per process, however many times this is called — a shared stand-in for
/// each call site's own `Once`-guarded `ensure_class`, but keyed per class
/// name rather than per call site.
///
/// # Safety
/// `wndproc` must be a valid window procedure (or `None`).
pub unsafe fn register_class(name: &str, wndproc: WNDPROC) {
    let registry = registered();
    let mut guard = registry.lock().unwrap_or_else(|e| e.into_inner());
    if !guard.insert(name.to_string()) {
        return;
    }
    let class_name = encode_wide(name);
    let icon = unsafe { LoadIconW(app_instance(), std::ptr::without_provenance(1)) };
    let wc = WNDCLASSEXW {
        cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
        style: CS_HREDRAW | CS_VREDRAW,
        lpfnWndProc: wndproc,
        cbClsExtra: 0,
        cbWndExtra: 0,
        hInstance: app_instance(),
        hIcon: icon,
        hCursor: unsafe { LoadCursorW(null_mut(), IDC_ARROW) },
        hbrBackground: null_mut(),
        lpszMenuName: std::ptr::null(),
        lpszClassName: class_name.as_ptr(),
        hIconSm: icon,
    };
    unsafe {
        RegisterClassExW(&wc);
    }
}
