//! Crash recovery and the show/hide state machine (shell-cloak / DWM-cloak /
//! forced-minimize), in one file because they share the same state bits.

use windows_sys::Win32::Foundation::{BOOL, HWND, LPARAM};
use windows_sys::Win32::Graphics::Dwm::{DwmSetWindowAttribute, DWMWA_CLOAK};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    EnumWindows, IsIconic, SetWindowPos, ShowWindow, SystemParametersInfoW, ANIMATIONINFO,
    SPI_GETANIMATION, SPI_SETANIMATION, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SWP_NOZORDER,
    SWP_SHOWWINDOW, SW_FORCEMINIMIZE, SW_HIDE, SW_SHOWMINNOACTIVE, SW_SHOWNA, SW_SHOWNOACTIVATE,
};
use winspaces_common::log_warn;

use super::state::{
    get_window_state, set_window_state, WINSPACES_STATE_CLOAKED, WINSPACES_STATE_FORCED_MINIMIZED,
    WINSPACES_STATE_HIDDEN_MASK, WINSPACES_STATE_SHELL_CLOAKED, WINSPACES_STATE_SYSTEM_HIDDEN,
    WINSPACES_STATE_WAS_ICONIC,
};

/// Restore every top-level window still carrying a WinSpaces state prop and
/// clear the prop. Runs at startup (recovers windows stranded by a crashed
/// instance — DWM cloaks and props outlive the process) and on clean exit.
pub fn reclaim_orphaned_windows() {
    unsafe extern "system" fn enum_proc(hwnd: HWND, _lparam: LPARAM) -> BOOL {
        let state = get_window_state(hwnd);
        if state == 0 {
            return 1;
        }
        if (state & WINSPACES_STATE_SYSTEM_HIDDEN) != 0 {
            let mut zero: i32 = 0;
            DwmSetWindowAttribute(
                hwnd,
                DWMWA_CLOAK as _,
                &mut zero as *mut _ as _,
                std::mem::size_of::<i32>() as u32,
            );
            ShowWindow(hwnd, SW_SHOWNOACTIVATE);
        } else if (state & WINSPACES_STATE_HIDDEN_MASK) != 0 {
            set_window_visibility(hwnd, true, false);
        }
        set_window_state(hwnd, 0);
        1
    }
    unsafe {
        EnumWindows(Some(enum_proc), 0);
    }
}

/// Disables the OS minimize/restore animation while alive and restores the
/// user's setting on drop. `SW_FORCEMINIMIZE` already skips the animation on
/// the hide side, but the `SW_SHOWNOACTIVATE`/`SW_SHOWMAXIMIZED` restores of a
/// show pass would each play it. Session-only (`fWinIni = 0`): a crash while
/// the guard is alive costs at most the current session's animation setting,
/// never the user's profile.
pub(crate) struct AnimationGuard {
    saved: Option<ANIMATIONINFO>,
}

impl AnimationGuard {
    pub(crate) fn new() -> Self {
        unsafe {
            let mut info: ANIMATIONINFO = std::mem::zeroed();
            info.cbSize = std::mem::size_of::<ANIMATIONINFO>() as u32;
            if SystemParametersInfoW(SPI_GETANIMATION, info.cbSize, &mut info as *mut _ as _, 0)
                != 0
                && info.iMinAnimate != 0
            {
                let saved = info;
                info.iMinAnimate = 0;
                SystemParametersInfoW(SPI_SETANIMATION, info.cbSize, &mut info as *mut _ as _, 0);
                return Self { saved: Some(saved) };
            }
            Self { saved: None }
        }
    }
}

impl Drop for AnimationGuard {
    fn drop(&mut self) {
        if let Some(mut info) = self.saved.take() {
            unsafe {
                SystemParametersInfoW(SPI_SETANIMATION, info.cbSize, &mut info as *mut _ as _, 0);
            }
        }
    }
}

/// `hwnd` is an opaque Win32 handle; every call below (`DwmSetWindowAttribute`,
/// `ShowWindow`, `IsIconic`, ...) tolerates a stale or invalid one by failing
/// gracefully, so this stays a safe fn despite carrying a raw-pointer-typed
/// parameter.
#[allow(clippy::not_unsafe_ptr_arg_deref)]
pub fn set_window_visibility(hwnd: HWND, visible: bool, show_all_taskbar: bool) {
    unsafe {
        let mut state = get_window_state(hwnd);
        if state == 0 {
            return;
        }

        if visible {
            let was_forced = (state & WINSPACES_STATE_FORCED_MINIMIZED) != 0;
            let was_cloaked = (state & WINSPACES_STATE_CLOAKED) != 0;
            let was_iconic = (state & WINSPACES_STATE_WAS_ICONIC) != 0;
            let was_shell = (state & WINSPACES_STATE_SHELL_CLOAKED) != 0;

            if was_shell {
                if winspaces_win32::shell_cloak::set_shell_cloak(hwnd, false) {
                    state &= !WINSPACES_STATE_SHELL_CLOAKED;
                } else {
                    // Keep the bit so the next show retries and recovery
                    // passes still know the window is shell-cloaked.
                    log_warn!("Shell uncloak failed for hwnd {:?}", hwnd);
                }
            }

            if was_cloaked {
                let mut zero: i32 = 0;
                DwmSetWindowAttribute(
                    hwnd,
                    DWMWA_CLOAK as _,
                    &mut zero as *mut _ as _,
                    std::mem::size_of::<i32>() as u32,
                );
                state &= !WINSPACES_STATE_CLOAKED;
                // A window that stays minimized needs no recompose nudge —
                // SWP_SHOWWINDOW would pop it fully visible for a frame
                // before SW_SHOWMINNOACTIVE below re-minimizes it.
                if !was_iconic {
                    SetWindowPos(
                        hwnd,
                        std::ptr::null_mut(),
                        0,
                        0,
                        0,
                        0,
                        SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE | SWP_SHOWWINDOW,
                    );
                }
            }

            if was_iconic {
                // A shell-cloaked window was never minimized by us; after the
                // uncloak it is already in the right (iconic) state.
                if !was_shell {
                    ShowWindow(hwnd, SW_SHOWMINNOACTIVE);
                }
            } else if was_forced {
                let mut wp: windows_sys::Win32::UI::WindowsAndMessaging::WINDOWPLACEMENT =
                    std::mem::zeroed();
                wp.length = std::mem::size_of::<
                    windows_sys::Win32::UI::WindowsAndMessaging::WINDOWPLACEMENT,
                >() as u32;
                if windows_sys::Win32::UI::WindowsAndMessaging::GetWindowPlacement(hwnd, &mut wp)
                    != 0
                    && (wp.flags & 0x0002) != 0
                {
                    // No non-activating maximize verb exists; the one
                    // deliberate activation at the end of switch_desktop
                    // still wins because it runs after the show pass.
                    ShowWindow(
                        hwnd,
                        windows_sys::Win32::UI::WindowsAndMessaging::SW_SHOWMAXIMIZED,
                    );
                } else {
                    ShowWindow(hwnd, SW_SHOWNOACTIVATE);
                }
                state &= !WINSPACES_STATE_FORCED_MINIMIZED;
            } else if !was_cloaked && !was_shell {
                // Reached only via the SW_HIDE fallback (windows the cloak
                // call rejected, e.g. elevated ones); the cloak paths need
                // no ShowWindow — the window stayed WS_VISIBLE throughout.
                ShowWindow(hwnd, SW_SHOWNA);
            }
        } else {
            if (state & WINSPACES_STATE_HIDDEN_MASK) != 0 {
                return;
            }

            if IsIconic(hwnd) != 0 {
                state |= WINSPACES_STATE_WAS_ICONIC;
            } else {
                state &= !WINSPACES_STATE_WAS_ICONIC;
            }

            if show_all_taskbar {
                // Shell cloak keeps the taskbar button and the app never
                // observes a minimize; forced minimize is the fallback for
                // builds where the undocumented interface is gone.
                if winspaces_win32::shell_cloak::set_shell_cloak(hwnd, true) {
                    state |= WINSPACES_STATE_SHELL_CLOAKED;
                } else if IsIconic(hwnd) == 0 {
                    ShowWindow(hwnd, SW_FORCEMINIMIZE);
                    state |= WINSPACES_STATE_FORCED_MINIMIZED;
                }
            } else {
                let mut one: i32 = 1;
                let hr = DwmSetWindowAttribute(
                    hwnd,
                    DWMWA_CLOAK as _,
                    &mut one as *mut _ as _,
                    std::mem::size_of::<i32>() as u32,
                );
                if hr >= 0 {
                    state |= WINSPACES_STATE_CLOAKED;
                } else {
                    ShowWindow(hwnd, SW_HIDE);
                }
            }
        }
        set_window_state(hwnd, state);
    }
}
