//! Per-HWND identity cache for the expensive, immutable window facts.
//!
//! `get_window_aumid` creates a COM `IPropertyStore` per call and
//! `get_process_image_path` opens a kernel handle; both ran for every tracked
//! window on every 5 s shadow tick even though AUMID, exe path and class are
//! fixed for the life of an HWND. Titles change constantly and stay out.
//!
//! Win32 recycles handle values and the destroy hook can be missed, so every
//! hit is validated against the owning pid: a recycled handle misses and
//! re-queries. Apps that change their AUMID mid-life via
//! `SetWindowAppUserModelID` exist but are rare, and the damage is bounded to
//! a stale matcher string in one snapshot.
//!
//! Entries exist only for tracked windows (the single caller is the workspace
//! capture, which walks the tracked set) and are dropped on
//! `SpaceManager::remove_window` / `prune_dead_windows`, so the map cannot
//! outgrow the tracked set.

use std::cell::RefCell;
use std::collections::HashMap;

use windows_sys::Win32::Foundation::HWND;
use windows_sys::Win32::UI::WindowsAndMessaging::GetWindowThreadProcessId;

use super::query::{get_process_image_path, get_window_aumid, get_window_class};

#[derive(Clone)]
pub struct WindowIdentity {
    pub aumid: String,
    pub exe_path: String,
    pub class_name: String,
}

struct Entry {
    pid: u32,
    identity: WindowIdentity,
}

thread_local! {
    static CACHE: RefCell<HashMap<isize, Entry>> = RefCell::new(HashMap::new());
}

/// # Safety
/// `hwnd` is an opaque Win32 handle; the underlying queries tolerate a stale
/// or invalid one by failing gracefully.
pub unsafe fn window_identity(hwnd: HWND) -> WindowIdentity {
    let mut pid = 0u32;
    GetWindowThreadProcessId(hwnd, &mut pid);
    let key = hwnd as isize;
    CACHE.with(|c| {
        let mut map = c.borrow_mut();
        if pid != 0 {
            if let Some(e) = map.get(&key) {
                if e.pid == pid {
                    return e.identity.clone();
                }
            }
        }
        let identity = WindowIdentity {
            aumid: get_window_aumid(hwnd),
            exe_path: get_process_image_path(hwnd),
            class_name: get_window_class(hwnd),
        };
        map.insert(
            key,
            Entry {
                pid,
                identity: identity.clone(),
            },
        );
        identity
    })
}

pub fn invalidate_identity(hwnd: HWND) {
    CACHE.with(|c| {
        c.borrow_mut().remove(&(hwnd as isize));
    });
}
