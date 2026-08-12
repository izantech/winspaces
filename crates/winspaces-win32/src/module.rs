//! Process module handle and OS build-number helpers.

use std::sync::OnceLock;

use windows_sys::Win32::Foundation::HMODULE;

/// The current process's module handle, used as `hInstance` for
/// `CreateWindowExW` / `RegisterClassW` across every owner-drawn surface.
pub fn app_instance() -> HMODULE {
    unsafe { windows_sys::Win32::System::LibraryLoader::GetModuleHandleW(std::ptr::null_mut()) }
}

/// The running Windows build number (`RtlGetVersion`), cached after first
/// call. Used to gate acrylic/Mica and other build-dependent behavior.
pub fn win_build() -> u32 {
    static BUILD: OnceLock<u32> = OnceLock::new();
    *BUILD.get_or_init(|| unsafe {
        type RtlGetVersionFn = unsafe extern "system" fn(
            *mut windows_sys::Win32::System::SystemInformation::OSVERSIONINFOW,
        ) -> i32;
        let ntdll = windows_sys::Win32::System::LibraryLoader::GetModuleHandleW(
            crate::text::encode_wide("ntdll.dll").as_ptr(),
        );
        if !ntdll.is_null() {
            if let Some(proc_addr) = windows_sys::Win32::System::LibraryLoader::GetProcAddress(
                ntdll,
                c"RtlGetVersion".as_ptr() as _,
            ) {
                let rtl_get_version: RtlGetVersionFn = std::mem::transmute(proc_addr);
                let mut vi: windows_sys::Win32::System::SystemInformation::OSVERSIONINFOW =
                    std::mem::zeroed();
                vi.dwOSVersionInfoSize = std::mem::size_of_val(&vi) as u32;
                if rtl_get_version(&mut vi) == 0 {
                    return vi.dwBuildNumber;
                }
            }
        }
        0
    })
}
