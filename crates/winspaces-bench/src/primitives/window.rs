//! Window discovery and identity probes: the eligibility checks and the
//! metadata lookups the daemon runs once per relevant event, read-only
//! against live windows that are never touched.

use windows_sys::Win32::Foundation::HWND;
use windows_sys::Win32::UI::WindowsAndMessaging::{EnumWindows, GetShellWindow};
use winspaces_core::spaces::{is_framed_window, is_tileable_window, is_valid_window};
use winspaces_core::workspaces::{
    get_process_image_path, get_window_aumid, get_window_class, get_window_title,
};

use crate::timing::Runner;

/// The windows this group probes: the shell's own top-level window, and the
/// first `is_valid_window` hit of a full `EnumWindows` walk.
pub struct Targets {
    pub shell: Option<HWND>,
    pub app: Option<HWND>,
}

unsafe extern "system" fn find_first_valid_proc(hwnd: HWND, lparam: isize) -> i32 {
    let out = &mut *(lparam as *mut Option<HWND>);
    if is_valid_window(hwnd) {
        *out = Some(hwnd);
        return 0; // stop enumeration: one hit is enough
    }
    1
}

pub fn discover() -> Targets {
    let shell = unsafe {
        let hwnd = GetShellWindow();
        if hwnd.is_null() {
            None
        } else {
            Some(hwnd)
        }
    };

    let mut app: Option<HWND> = None;
    unsafe {
        EnumWindows(Some(find_first_valid_proc), &mut app as *mut _ as isize);
    }

    Targets { shell, app }
}

const APP_BENCH_NAMES: [&str; 7] = [
    "win32/is_valid_window/app",
    "win32/is_framed_window/app",
    "win32/is_tileable_window/app",
    "win32/get_window_title/app",
    "win32/get_window_class/app",
    "win32/get_process_image_path/app",
    "win32/get_window_aumid/app",
];

pub fn bench(r: &mut Runner, targets: &Targets) {
    match targets.shell {
        Some(hwnd) => r.bench("win32/is_valid_window/shell", || {
            std::hint::black_box(is_valid_window(hwnd));
        }),
        None => r.skip(
            "win32/is_valid_window/shell",
            "GetShellWindow returned no window",
        ),
    }

    let Some(app) = targets.app else {
        for name in APP_BENCH_NAMES {
            r.skip(name, "no is_valid_window hit found by EnumWindows");
        }
        return;
    };

    r.bench("win32/is_valid_window/app", || {
        std::hint::black_box(is_valid_window(app));
    });
    r.bench("win32/is_framed_window/app", || {
        std::hint::black_box(is_framed_window(app));
    });
    r.bench("win32/is_tileable_window/app", || {
        std::hint::black_box(is_tileable_window(app));
    });
    r.bench("win32/get_window_title/app", || {
        std::hint::black_box(unsafe { get_window_title(app) });
    });
    r.bench("win32/get_window_class/app", || {
        std::hint::black_box(unsafe { get_window_class(app) });
    });
    r.bench("win32/get_process_image_path/app", || {
        std::hint::black_box(unsafe { get_process_image_path(app) });
    });
    r.bench("win32/get_window_aumid/app", || {
        std::hint::black_box(unsafe { get_window_aumid(app) });
    });
}
