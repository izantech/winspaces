//! Shell-level window cloaking via the undocumented ImmersiveShell
//! `IApplicationView::SetCloak` — the same mechanism native virtual desktops
//! use to hide windows on other desktops. Unlike `DWMWA_CLOAK`, the taskbar
//! keeps buttons for windows cloaked this way, which is exactly the contract
//! of the "show all windows on taskbar" mode, and the app never observes a
//! minimize (Chromium keeps its last frame instead of white-flashing on
//! restore).
//!
//! The interface is undocumented COM, so every caller must tolerate failure
//! (unknown Windows build, Explorer not running) and fall back to the
//! documented `SW_FORCEMINIMIZE` path. IIDs and vtable layouts follow the
//! MIT-licensed AltTabAccessor reference; the argument values are the ones
//! the shell itself uses: `SetCloak(1, 2)` cloaks, `SetCloak(1, 0)` uncloaks.
//!
//! Requires COM initialized on the calling thread (the daemon thread does
//! `CoInitializeEx(COINIT_APARTMENTTHREADED)` at startup).

use std::cell::Cell;
use std::ffi::c_void;
use std::ptr::null_mut;
use std::sync::OnceLock;

use windows_sys::core::{GUID, HRESULT};
use windows_sys::Win32::Foundation::HWND;
use windows_sys::Win32::System::Com::{CoCreateInstance, CLSCTX_ALL};
use winspaces_common::{log_info, log_warn};

const CLSID_IMMERSIVE_SHELL: GUID = GUID {
    data1: 0xC2F03A33,
    data2: 0x21F5,
    data3: 0x47FA,
    data4: [0xB4, 0xBB, 0x15, 0x63, 0x62, 0xA2, 0xF2, 0x39],
};

const IID_SERVICE_PROVIDER: GUID = GUID {
    data1: 0x6D5140C1,
    data2: 0x7436,
    data3: 0x11CE,
    data4: [0x80, 0x34, 0x00, 0xAA, 0x00, 0x60, 0x09, 0xFA],
};

const IID_APPLICATION_VIEW_COLLECTION: GUID = GUID {
    data1: 0x1841C6D7,
    data2: 0x4F9D,
    data3: 0x42C0,
    data4: [0xAF, 0x41, 0x87, 0x47, 0x53, 0x8F, 0x10, 0xE5],
};

#[repr(C)]
struct IUnknownVtbl {
    query_interface:
        unsafe extern "system" fn(*mut c_void, *const GUID, *mut *mut c_void) -> HRESULT,
    add_ref: unsafe extern "system" fn(*mut c_void) -> u32,
    release: unsafe extern "system" fn(*mut c_void) -> u32,
}

#[repr(C)]
struct IServiceProviderVtbl {
    base: IUnknownVtbl,
    query_service: unsafe extern "system" fn(
        *mut c_void,
        *const GUID,
        *const GUID,
        *mut *mut c_void,
    ) -> HRESULT,
}

/// Slots 3-5 are view enumerators (`GetViews`, by z-order, by AUMID) that are
/// never called; only their vtable positions matter.
#[repr(C)]
struct IApplicationViewCollectionVtbl {
    base: IUnknownVtbl,
    unused: [*const c_void; 3],
    get_view_for_hwnd: unsafe extern "system" fn(*mut c_void, HWND, *mut *mut c_void) -> HRESULT,
}

/// Slots 3-5 are the IInspectable methods, 6-11 view methods (`SetFocus`
/// through `GetVisibility`) that are never called; `SetCloak` is slot 12.
#[repr(C)]
struct IApplicationViewVtbl {
    base: IUnknownVtbl,
    unused: [*const c_void; 9],
    set_cloak: unsafe extern "system" fn(*mut c_void, u32, i32) -> HRESULT,
}

unsafe fn release(ptr: *mut c_void) {
    unsafe {
        let vtbl = *(ptr as *mut *const IUnknownVtbl);
        ((*vtbl).release)(ptr);
    }
}

thread_local! {
    /// Cached `IApplicationViewCollection`, resolved lazily and re-resolved
    /// after a failed call (Explorer restarts invalidate the proxy).
    static COLLECTION: Cell<*mut c_void> = const { Cell::new(null_mut()) };
    static UNAVAILABLE_LOGGED: Cell<bool> = const { Cell::new(false) };
}

/// Escape hatch to force the fallback path, so the forced-minimize backend
/// stays testable on machines where the shell cloak works.
fn disabled_by_env() -> bool {
    static DISABLED: OnceLock<bool> = OnceLock::new();
    *DISABLED.get_or_init(|| std::env::var_os("WINSPACES_NO_SHELL_CLOAK").is_some_and(|v| v != "0"))
}

/// `CLSID_ImmersiveShell` → `IServiceProvider` → `IApplicationViewCollection`.
/// Null on any failure; the shell passes the interface IID as the service
/// GUID too.
fn resolve_collection() -> *mut c_void {
    unsafe {
        let mut provider: *mut c_void = null_mut();
        let hr = CoCreateInstance(
            &CLSID_IMMERSIVE_SHELL,
            null_mut(),
            CLSCTX_ALL,
            &IID_SERVICE_PROVIDER,
            &mut provider,
        );
        if hr < 0 || provider.is_null() {
            return null_mut();
        }
        let vtbl = *(provider as *mut *const IServiceProviderVtbl);
        let mut collection: *mut c_void = null_mut();
        let hr = ((*vtbl).query_service)(
            provider,
            &IID_APPLICATION_VIEW_COLLECTION,
            &IID_APPLICATION_VIEW_COLLECTION,
            &mut collection,
        );
        release(provider);
        if hr < 0 || collection.is_null() {
            return null_mut();
        }
        collection
    }
}

unsafe fn try_set_cloak(collection: *mut c_void, hwnd: HWND, cloak: bool) -> bool {
    unsafe {
        let vtbl = *(collection as *mut *const IApplicationViewCollectionVtbl);
        let mut view: *mut c_void = null_mut();
        let hr = ((*vtbl).get_view_for_hwnd)(collection, hwnd, &mut view);
        if hr < 0 || view.is_null() {
            return false;
        }
        let view_vtbl = *(view as *mut *const IApplicationViewVtbl);
        let hr = ((*view_vtbl).set_cloak)(view, 1, if cloak { 2 } else { 0 });
        release(view);
        hr >= 0
    }
}

/// Shell-cloak (`cloak = true`) or uncloak a window. Returns `false` when the
/// backend is unavailable or the shell has no view for the window, so the
/// caller can fall back to forced minimize.
///
/// `hwnd` is an opaque Win32 handle, never dereferenced in Rust — it is only
/// ever passed on to `IApplicationViewCollection::GetViewForHwnd`, so this
/// stays a safe fn despite carrying a raw-pointer-typed parameter.
#[allow(clippy::not_unsafe_ptr_arg_deref)]
pub fn set_shell_cloak(hwnd: HWND, cloak: bool) -> bool {
    if disabled_by_env() {
        return false;
    }
    COLLECTION.with(|cell| {
        let mut collection = cell.get();
        if collection.is_null() {
            collection = resolve_collection();
            cell.set(collection);
            if collection.is_null() {
                UNAVAILABLE_LOGGED.with(|logged| {
                    if !logged.get() {
                        logged.set(true);
                        log_warn!(
                            "Shell cloak backend unavailable; falling back to forced minimize"
                        );
                    }
                });
                return false;
            }
            log_info!("Shell cloak backend resolved");
        }
        if unsafe { try_set_cloak(collection, hwnd, cloak) } {
            return true;
        }
        // A stale proxy (Explorer restarted) fails every call: re-resolve
        // once and retry before reporting failure.
        unsafe { release(collection) };
        collection = resolve_collection();
        cell.set(collection);
        if collection.is_null() {
            return false;
        }
        unsafe { try_set_cloak(collection, hwnd, cloak) }
    })
}
