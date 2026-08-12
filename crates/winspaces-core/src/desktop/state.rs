//! The process-external `SetProp` contract: how a window's WinSpaces state
//! survives as a Win32 property, independent of our own process lifetime.
//! Kept in one file because crash recovery (`reclaim_orphaned_windows`) reads
//! these bits back from windows a *previous* instance tagged.

use std::ffi::c_void;
use windows_sys::Win32::Foundation::HWND;
use windows_sys::Win32::UI::WindowsAndMessaging::{GetPropA, RemovePropA, SetPropA};

pub(crate) const WINSPACES_PROP_STATE: &[u8] = b"WinSpacesWindowState\0";
/// How long a completed scan exempts `switch_desktop` from running another
/// one. Rapid space-stepping would otherwise pay a full `EnumWindows` (with a
/// cross-process DWM probe per window) on every hop.
pub(crate) const SCAN_THROTTLE_MS: u32 = 250;

pub(crate) const WINSPACES_STATE_TRACKED: usize = 0x01;
pub(crate) const WINSPACES_STATE_WAS_ICONIC: usize = 0x02;
pub(crate) const WINSPACES_STATE_FORCED_MINIMIZED: usize = 0x04;
pub(crate) const WINSPACES_STATE_CLOAKED: usize = 0x08;
// System windows (input experience, task host, ...) that we cloaked out of the
// way. Marked so exit/startup passes can undo the cloak: DWM cloaks persist
// after the process that applied them dies.
pub(crate) const WINSPACES_STATE_SYSTEM_HIDDEN: usize = 0x10;
// Hidden via the ImmersiveShell cloak (`shell_cloak.rs`). A DWM uncloak does
// NOT clear this kind of cloak — recovery must go through the same COM call.
pub(crate) const WINSPACES_STATE_SHELL_CLOAKED: usize = 0x20;

/// Every state bit that means "we hid this window". Shared by the eligibility
/// probe, the scan skip, crash recovery, and the hide early-return so a new
/// hiding backend cannot be forgotten in one of them.
pub(crate) const WINSPACES_STATE_HIDDEN_MASK: usize =
    WINSPACES_STATE_CLOAKED | WINSPACES_STATE_FORCED_MINIMIZED | WINSPACES_STATE_SHELL_CLOAKED;

pub(crate) fn get_window_state(hwnd: HWND) -> usize {
    unsafe { GetPropA(hwnd, WINSPACES_PROP_STATE.as_ptr()) as usize }
}

pub(crate) fn set_window_state(hwnd: HWND, state: usize) {
    unsafe {
        if state == 0 {
            RemovePropA(hwnd, WINSPACES_PROP_STATE.as_ptr());
        } else {
            SetPropA(hwnd, WINSPACES_PROP_STATE.as_ptr(), state as *mut c_void);
        }
    }
}
