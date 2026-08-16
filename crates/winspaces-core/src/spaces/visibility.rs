//! Crash recovery and the show/hide state machine (shell-cloak / DWM-cloak /
//! forced-minimize), in one file because they share the same state bits.

use windows_sys::Win32::Foundation::{BOOL, HWND, LPARAM};
use windows_sys::Win32::Graphics::Dwm::{
    DwmGetWindowAttribute, DwmSetWindowAttribute, DWMWA_CLOAK, DWMWA_CLOAKED,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    EnumWindows, IsIconic, SetWindowPos, ShowWindow, SystemParametersInfoW, ANIMATIONINFO,
    SPI_GETANIMATION, SPI_SETANIMATION, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SWP_NOZORDER,
    SWP_SHOWWINDOW, SW_FORCEMINIMIZE, SW_HIDE, SW_SHOWMINNOACTIVE, SW_SHOWNA, SW_SHOWNOACTIVATE,
};
use winspaces_common::log_warn;

use super::state::{
    get_window_state, set_window_state, WINSPACES_STATE_CLOAKED, WINSPACES_STATE_FORCED_MINIMIZED,
    WINSPACES_STATE_HIDDEN_MASK, WINSPACES_STATE_SHELL_CLOAKED, WINSPACES_STATE_SW_HIDDEN,
    WINSPACES_STATE_SYSTEM_HIDDEN, WINSPACES_STATE_WAS_ICONIC,
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
            // Uncloak, but never re-show. The scan hides one of these only
            // when it is genuinely on screen, which for a window on that list
            // is an anomaly rather than a state worth preserving — and the
            // `SW_SHOWNOACTIVATE` that used to run here is what created the
            // anomaly in the first place: an unowned, non-tool window handed
            // `WS_VISIBLE` gets a taskbar button, so every exit left "Task
            // Host Window" and "Windows Push Notifications Platform" sitting
            // in the taskbar, and the next scan then read that as their
            // baseline and restored it again. Leaving them hidden ends the
            // loop and repairs a session an older build corrupted; anything
            // that genuinely needs its window back is a system component that
            // shows it on demand.
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
            let was_sw_hidden = (state & WINSPACES_STATE_SW_HIDDEN) != 0;

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
                let hr = DwmSetWindowAttribute(
                    hwnd,
                    DWMWA_CLOAK as _,
                    &mut zero as *mut _ as _,
                    std::mem::size_of::<i32>() as u32,
                );
                if hr >= 0 {
                    state &= !WINSPACES_STATE_CLOAKED;
                } else {
                    // Same reasoning as the shell branch above: keep the bit
                    // so the next show retries and recovery still knows we
                    // cloaked it. Clearing it on a window that is still
                    // cloaked is unrecoverable (see the verify below).
                    log_warn!("DWM uncloak failed for hwnd {:?} (hr {:#x})", hwnd, hr);
                }
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

            // Trust the window, not the return codes. Both uncloak calls can
            // report success and leave the window cloaked anyway: a stale
            // ImmersiveShell proxy (Explorer restarted) answers `SetCloak`
            // with S_OK without doing anything, and a taskbar-mode switch can
            // leave a shell cloak underneath the DWM cloak it replaced it
            // with — the DWM uncloak then clears only its own layer.
            //
            // Dropping the bits here would strand the window for good: still
            // invisible, but now reading as *externally* cloaked, so it fails
            // `is_valid_window` forever and no later show, switch, capture or
            // `reclaim_orphaned_windows` pass will ever touch it again. Only
            // `scripts/recover-windows.ps1` could bring it back. Re-assert
            // whichever layer is still on so the next show retries it.
            if was_cloaked || was_shell {
                let mut still: u32 = 0;
                let probe = DwmGetWindowAttribute(
                    hwnd,
                    DWMWA_CLOAKED as _,
                    &mut still as *mut _ as _,
                    std::mem::size_of::<u32>() as u32,
                );
                if probe == 0 && still != 0 {
                    // DWM_CLOAKED_SHELL (0x2) means the shell cloak survived;
                    // anything else is our own DWM layer (APP) or an owner's
                    // inherited cloak, which the DWM call is what clears.
                    let bit = if still == 0x2 {
                        WINSPACES_STATE_SHELL_CLOAKED
                    } else {
                        WINSPACES_STATE_CLOAKED
                    };
                    state |= bit;
                    log_warn!(
                        "hwnd {:?} still cloaked after uncloak (DWMWA_CLOAKED={:#x}); keeping state {:#x} for retry",
                        hwnd,
                        still,
                        bit
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
                    // deliberate activation at the end of switch_space
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

            // Cleared last: `was_iconic` above re-minimizes instead, and that
            // path must still drop the bit or the window stays "hidden by us"
            // forever.
            if was_sw_hidden {
                state &= !WINSPACES_STATE_SW_HIDDEN;
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
                    // Elevated windows refuse `DWMWA_CLOAK` even from an
                    // elevated daemon. `SW_HIDE` is the fallback, and it is
                    // the only backend that clears `WS_VISIBLE` — so it must
                    // record a bit, or the eligibility probe reads the window
                    // as an ordinary invisible one and the show pass never
                    // touches it again.
                    log_warn!(
                        "DWM cloak failed for hwnd {:?} (hr {:#x}); hiding with SW_HIDE",
                        hwnd,
                        hr
                    );
                    ShowWindow(hwnd, SW_HIDE);
                    state |= WINSPACES_STATE_SW_HIDDEN;
                }
            }
        }
        set_window_state(hwnd, state);
    }
}
