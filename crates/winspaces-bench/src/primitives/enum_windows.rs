//! `win32/enum_valid_windows`: a full `EnumWindows` walk filtered by
//! eligibility — the scan floor behind every retile and reconcile.

use windows_sys::Win32::Foundation::HWND;
use windows_sys::Win32::UI::WindowsAndMessaging::EnumWindows;
use winspaces_core::spaces::is_valid_window;

use crate::timing::Runner;

unsafe extern "system" fn collect_valid_proc(hwnd: HWND, lparam: isize) -> i32 {
    let out = &mut *(lparam as *mut Vec<HWND>);
    if is_valid_window(hwnd) {
        out.push(hwnd);
    }
    1
}

pub fn bench(r: &mut Runner) {
    let name = "win32/enum_valid_windows";
    if !r.selected(name) {
        return;
    }

    let mut buf: Vec<HWND> = Vec::new();
    unsafe {
        EnumWindows(Some(collect_valid_proc), &mut buf as *mut _ as isize);
    }
    eprintln!("  note: {name} found {} valid windows", buf.len());

    r.bench(name, || {
        buf.clear();
        unsafe {
            EnumWindows(Some(collect_valid_proc), &mut buf as *mut _ as isize);
        }
        std::hint::black_box(buf.len());
    });
}
