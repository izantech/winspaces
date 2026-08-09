use std::ptr::null_mut;
use windows_sys::Win32::Foundation::HWND;
use windows_sys::Win32::UI::Accessibility::{SetWinEventHook, UnhookWinEvent, HWINEVENTHOOK};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    EVENT_SYSTEM_FOREGROUND, WINEVENT_OUTOFCONTEXT, WINEVENT_SKIPOWNPROCESS,
};

pub struct WinEventHook {
    hook_handle: HWINEVENTHOOK,
}

impl WinEventHook {
    pub fn install(
        callback: unsafe extern "system" fn(HWINEVENTHOOK, u32, HWND, i32, i32, u32, u32),
    ) -> Option<Self> {
        let handle = unsafe {
            SetWinEventHook(
                EVENT_SYSTEM_FOREGROUND,
                EVENT_SYSTEM_FOREGROUND,
                null_mut(),
                Some(callback),
                0,
                0,
                WINEVENT_OUTOFCONTEXT | WINEVENT_SKIPOWNPROCESS,
            )
        };

        if !handle.is_null() {
            Some(Self {
                hook_handle: handle,
            })
        } else {
            None
        }
    }
}

impl Drop for WinEventHook {
    fn drop(&mut self) {
        if !self.hook_handle.is_null() {
            unsafe {
                UnhookWinEvent(self.hook_handle);
            }
        }
    }
}
