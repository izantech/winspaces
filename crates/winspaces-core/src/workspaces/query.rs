//! Process path, class, AUMID and title/placement lookups for a window.

use windows_sys::Win32::Foundation::{CloseHandle, HANDLE, HWND, RECT};
use windows_sys::Win32::System::Threading::{
    OpenProcess, QueryFullProcessImageNameW, PROCESS_QUERY_LIMITED_INFORMATION,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    GetClassNameW, GetWindowPlacement, GetWindowRect, GetWindowTextW, GetWindowThreadProcessId,
    WINDOWPLACEMENT,
};
use winspaces_common::WindowRect;

/// # Safety
/// `hwnd` is an opaque Win32 handle; the calls below tolerate a stale or
/// invalid one by failing gracefully.
pub unsafe fn get_process_image_path(hwnd: HWND) -> String {
    let mut pid = 0;
    GetWindowThreadProcessId(hwnd, &mut pid);
    if pid == 0 {
        return String::new();
    }

    let h_process: HANDLE = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
    if h_process.is_null() {
        return String::new();
    }

    let mut buf = [0u16; 1024];
    let mut size = buf.len() as u32;
    let mut path = String::new();

    if QueryFullProcessImageNameW(h_process, 0, buf.as_mut_ptr(), &mut size) != 0 {
        path = String::from_utf16_lossy(&buf[..size as usize]);
    }

    CloseHandle(h_process);
    path
}

/// # Safety
/// `hwnd` is an opaque Win32 handle; `GetClassNameW` tolerates a stale or
/// invalid one by failing gracefully.
pub unsafe fn get_window_class(hwnd: HWND) -> String {
    let mut buf = [0u16; 256];
    let len = GetClassNameW(hwnd, buf.as_mut_ptr(), buf.len() as i32);
    if len > 0 {
        String::from_utf16_lossy(&buf[..len as usize])
    } else {
        String::new()
    }
}

#[allow(non_camel_case_types, clippy::upper_case_acronyms)]
#[repr(C)]
struct PROPERTYKEY {
    fmtid: windows_sys::core::GUID,
    pid: u32,
}

#[allow(non_camel_case_types, clippy::upper_case_acronyms)]
#[repr(C)]
struct PROPVARIANT {
    vt: u16,
    w_reserved1: u16,
    w_reserved2: u16,
    w_reserved3: u16,
    val1: usize,
    val2: usize,
}

#[allow(non_snake_case)]
#[repr(C)]
struct IPropertyStoreVtbl {
    pub QueryInterface: unsafe extern "system" fn(
        this: *mut std::ffi::c_void,
        riid: *const windows_sys::core::GUID,
        ppv: *mut *mut std::ffi::c_void,
    ) -> i32,
    pub AddRef: unsafe extern "system" fn(this: *mut std::ffi::c_void) -> u32,
    pub Release: unsafe extern "system" fn(this: *mut std::ffi::c_void) -> u32,
    pub GetCount: unsafe extern "system" fn(this: *mut std::ffi::c_void, cprops: *mut u32) -> i32,
    pub GetAt: unsafe extern "system" fn(
        this: *mut std::ffi::c_void,
        iprop: u32,
        pkey: *mut PROPERTYKEY,
    ) -> i32,
    pub GetValue: unsafe extern "system" fn(
        this: *mut std::ffi::c_void,
        key: *const PROPERTYKEY,
        pv: *mut PROPVARIANT,
    ) -> i32,
    pub SetValue: unsafe extern "system" fn(
        this: *mut std::ffi::c_void,
        key: *const PROPERTYKEY,
        pv: *const PROPVARIANT,
    ) -> i32,
    pub Commit: unsafe extern "system" fn(this: *mut std::ffi::c_void) -> i32,
}

#[link(name = "shell32")]
extern "system" {
    fn SHGetPropertyStoreForWindow(
        hwnd: HWND,
        riid: *const windows_sys::core::GUID,
        ppv: *mut *mut std::ffi::c_void,
    ) -> i32;
}

use windows_sys::Win32::System::Com::CoTaskMemFree;

/// # Safety
/// `hwnd` is an opaque Win32 handle; `SHGetPropertyStoreForWindow` tolerates
/// a stale or invalid one by failing gracefully. The hand-rolled
/// `IPropertyStoreVtbl` calls trust that a non-null `store_ptr` returned by
/// that API points at a valid `IPropertyStore` COM object, per its contract.
pub unsafe fn get_window_aumid(hwnd: HWND) -> String {
    const IID_IPROPERTYSTORE: windows_sys::core::GUID = windows_sys::core::GUID {
        data1: 0x886d8eeb,
        data2: 0x8cf2,
        data3: 0x4446,
        data4: [0x8d, 0x02, 0xcd, 0xba, 0x1d, 0xbd, 0xcf, 0x99],
    };

    const PKEY_APPUSERMODEL_ID: PROPERTYKEY = PROPERTYKEY {
        fmtid: windows_sys::core::GUID {
            data1: 0x9F4C2855,
            data2: 0x9F79,
            data3: 0x4B39,
            data4: [0xA8, 0xD0, 0xE1, 0xD4, 0x2D, 0xE1, 0xD5, 0xF3],
        },
        pid: 5,
    };

    let mut store_ptr: *mut std::ffi::c_void = std::ptr::null_mut();
    if SHGetPropertyStoreForWindow(hwnd, &IID_IPROPERTYSTORE, &mut store_ptr) == 0
        && !store_ptr.is_null()
    {
        let vtbl = *(store_ptr as *mut *const IPropertyStoreVtbl);
        let mut pv: PROPVARIANT = std::mem::zeroed();
        let mut result = String::new();
        if ((*vtbl).GetValue)(store_ptr, &PKEY_APPUSERMODEL_ID, &mut pv) == 0
            && pv.vt == 31
            && pv.val1 != 0
        {
            // VT_LPWSTR = 31
            let ptr = pv.val1 as *const u16;
            let mut len = 0;
            while *ptr.add(len) != 0 {
                len += 1;
            }
            let slice = std::slice::from_raw_parts(ptr, len);
            result = String::from_utf16_lossy(slice);
            CoTaskMemFree(ptr as *const std::ffi::c_void);
        }
        ((*vtbl).Release)(store_ptr);
        return result;
    }

    String::new()
}

/// # Safety
/// `hwnd` is an opaque Win32 handle; `GetWindowTextW` tolerates a stale or
/// invalid one by failing gracefully.
pub unsafe fn get_window_title(hwnd: HWND) -> String {
    let mut buf = [0u16; 512];
    let len = GetWindowTextW(hwnd, buf.as_mut_ptr(), buf.len() as i32);
    if len > 0 {
        String::from_utf16_lossy(&buf[..len as usize])
    } else {
        String::new()
    }
}

/// # Safety
/// `hwnd` is an opaque Win32 handle; `GetWindowPlacement`/`GetWindowRect`
/// tolerate a stale or invalid one by failing gracefully.
pub unsafe fn get_window_placement_info(hwnd: HWND) -> (u32, WindowRect) {
    let mut wp: WINDOWPLACEMENT = std::mem::zeroed();
    wp.length = std::mem::size_of::<WINDOWPLACEMENT>() as u32;
    if GetWindowPlacement(hwnd, &mut wp) != 0 {
        let is_maximized = wp.showCmd
            == windows_sys::Win32::UI::WindowsAndMessaging::SW_SHOWMAXIMIZED as u32
            || (wp.flags & 0x0002) != 0; // WPF_RESTORETOMAXIMIZED

        let show_cmd = if is_maximized {
            windows_sys::Win32::UI::WindowsAndMessaging::SW_SHOWMAXIMIZED as u32
        } else {
            windows_sys::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL as u32
        };

        let rect =
            if wp.showCmd == windows_sys::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL as u32 {
                let mut r: RECT = std::mem::zeroed();
                if GetWindowRect(hwnd, &mut r) != 0 && r.right > r.left {
                    r
                } else {
                    wp.rcNormalPosition
                }
            } else {
                wp.rcNormalPosition
            };

        (
            show_cmd,
            WindowRect {
                left: rect.left,
                top: rect.top,
                right: rect.right,
                bottom: rect.bottom,
            },
        )
    } else {
        (
            windows_sys::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL as u32,
            WindowRect::default(),
        )
    }
}
