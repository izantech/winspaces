use crate::log_info;
use windows_sys::Win32::Foundation::HWND;
use windows_sys::Win32::UI::Accessibility::{SetWinEventHook, UnhookWinEvent, HWINEVENTHOOK};
use windows_sys::Win32::UI::WindowsAndMessaging::{WINEVENT_OUTOFCONTEXT, WINEVENT_SKIPOWNPROCESS};

pub const EVENT_SYSTEM_FOREGROUND: u32 = 0x0003;

#[allow(non_snake_case, clippy::upper_case_acronyms)]
pub type WINEVENTPROC = unsafe extern "system" fn(
    hWinEventHook: HWINEVENTHOOK,
    event: u32,
    hwnd: HWND,
    idObject: i32,
    idChild: i32,
    idEventThread: u32,
    dwmsEventTime: u32,
);

pub struct WinEventHook {
    hook: HWINEVENTHOOK,
}

impl WinEventHook {
    pub fn install(proc: WINEVENTPROC) -> Option<Self> {
        unsafe {
            let hook = SetWinEventHook(
                EVENT_SYSTEM_FOREGROUND,
                EVENT_SYSTEM_FOREGROUND,
                std::ptr::null_mut(),
                Some(proc),
                0,
                0,
                WINEVENT_OUTOFCONTEXT | WINEVENT_SKIPOWNPROCESS,
            );
            if !hook.is_null() {
                log_info!("Installed WinEvent foreground hook: {:?}", hook);
                Some(Self { hook })
            } else {
                log_info!("Failed to install WinEvent foreground hook");
                None
            }
        }
    }
}

impl Drop for WinEventHook {
    fn drop(&mut self) {
        unsafe {
            if !self.hook.is_null() {
                log_info!("Uninstalling WinEvent foreground hook: {:?}", self.hook);
                UnhookWinEvent(self.hook);
            }
        }
    }
}

pub struct KeyboardHook {
    hook: windows_sys::Win32::UI::WindowsAndMessaging::HHOOK,
}

impl KeyboardHook {
    pub fn install(proc: windows_sys::Win32::UI::WindowsAndMessaging::HOOKPROC) -> Option<Self> {
        unsafe {
            let hinst =
                windows_sys::Win32::System::LibraryLoader::GetModuleHandleW(std::ptr::null_mut());
            let hook = windows_sys::Win32::UI::WindowsAndMessaging::SetWindowsHookExW(
                windows_sys::Win32::UI::WindowsAndMessaging::WH_KEYBOARD_LL,
                proc,
                hinst,
                0,
            );
            if !hook.is_null() {
                log_info!("Installed low-level keyboard hook: {:?}", hook);
                Some(Self { hook })
            } else {
                log_info!("Failed to install low-level keyboard hook");
                None
            }
        }
    }
}

impl Drop for KeyboardHook {
    fn drop(&mut self) {
        unsafe {
            if !self.hook.is_null() {
                log_info!("Uninstalling low-level keyboard hook: {:?}", self.hook);
                windows_sys::Win32::UI::WindowsAndMessaging::UnhookWindowsHookEx(self.hook);
            }
        }
    }
}

#[allow(dead_code)]
pub struct MouseHook {
    hook: windows_sys::Win32::UI::WindowsAndMessaging::HHOOK,
}

#[allow(dead_code)]
impl MouseHook {
    pub fn install(proc: windows_sys::Win32::UI::WindowsAndMessaging::HOOKPROC) -> Option<Self> {
        unsafe {
            let hinst =
                windows_sys::Win32::System::LibraryLoader::GetModuleHandleW(std::ptr::null_mut());
            let hook = windows_sys::Win32::UI::WindowsAndMessaging::SetWindowsHookExW(
                windows_sys::Win32::UI::WindowsAndMessaging::WH_MOUSE_LL,
                proc,
                hinst,
                0,
            );
            if !hook.is_null() {
                log_info!("Installed low-level mouse hook: {:?}", hook);
                Some(Self { hook })
            } else {
                log_info!("Failed to install low-level mouse hook");
                None
            }
        }
    }
}

impl Drop for MouseHook {
    fn drop(&mut self) {
        unsafe {
            if !self.hook.is_null() {
                log_info!("Uninstalling low-level mouse hook: {:?}", self.hook);
                windows_sys::Win32::UI::WindowsAndMessaging::UnhookWindowsHookEx(self.hook);
            }
        }
    }
}
