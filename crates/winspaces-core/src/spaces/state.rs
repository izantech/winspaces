//! The process-external `SetProp` contract: how a window's WinSpaces state
//! survives as a Win32 property, independent of our own process lifetime.
//! Kept in one file because crash recovery (`reclaim_orphaned_windows`) reads
//! these bits back from windows a *previous* instance tagged.

use std::ffi::c_void;
use windows_sys::Win32::Foundation::HWND;
use windows_sys::Win32::UI::WindowsAndMessaging::{GetPropA, RemovePropA, SetPropA};

pub(crate) const WINSPACES_PROP_STATE: &[u8] = b"WinSpacesWindowState\0";
/// How long a completed scan exempts `switch_space` from running another
/// one. Rapid space-stepping would otherwise pay a full `EnumWindows` (with a
/// cross-process DWM probe per window) on every hop.
pub(crate) const SCAN_THROTTLE_MS: u32 = 250;

pub(crate) const WINSPACES_STATE_TRACKED: usize = 0x01;
pub(crate) const WINSPACES_STATE_WAS_ICONIC: usize = 0x02;
pub(crate) const WINSPACES_STATE_FORCED_MINIMIZED: usize = 0x04;
pub(crate) const WINSPACES_STATE_CLOAKED: usize = 0x08;
// System windows (input experience, task host, ...) that we hid out of the
// way. Marked so exit/startup passes can undo the cloak: DWM cloaks persist
// after the process that applied them dies. The visibility is deliberately
// *not* undone with it — see `system_window_is_showing`.
pub(crate) const WINSPACES_STATE_SYSTEM_HIDDEN: usize = 0x10;
// Hidden via the ImmersiveShell cloak (`shell_cloak.rs`). A DWM uncloak does
// NOT clear this kind of cloak — recovery must go through the same COM call.
pub(crate) const WINSPACES_STATE_SHELL_CLOAKED: usize = 0x20;
// Hidden with `ShowWindow(SW_HIDE)` because `DWMWA_CLOAK` was rejected —
// elevated windows are the common case, and they refuse the cloak even when
// the daemon itself is elevated.
//
// This needs its own bit precisely because it is the one hiding backend that
// clears `WS_VISIBLE`. The eligibility probe treats an invisible window as
// unmanageable *unless* a hidden bit claims it, so a fallback that recorded
// nothing left the window failing `is_valid_window` forever: the show pass
// skips it before reaching the `SW_SHOWNA` that would bring it back, and it
// drops out of captures. Invisible, tracked, and unreachable.
pub(crate) const WINSPACES_STATE_SW_HIDDEN: usize = 0x40;
// 0x80 is deliberately free. A sticky/pinned bit lived here briefly and was
// removed: nothing could read it back, because `SpaceManager::new` runs
// `reclaim_orphaned_windows` — which zeroes every prop it finds — before the
// first scan. It only ever added a second source of truth to drift from
// `SpaceManager::sticky_windows`. Pins persist through the layout shadow's
// `WindowSnapshot::is_sticky` instead.

/// Every state bit that means "we hid this window". Shared by the eligibility
/// probe, the scan skip, crash recovery, and the hide early-return so a new
/// hiding backend cannot be forgotten in one of them.
pub(crate) const WINSPACES_STATE_HIDDEN_MASK: usize = WINSPACES_STATE_CLOAKED
    | WINSPACES_STATE_FORCED_MINIMIZED
    | WINSPACES_STATE_SHELL_CLOAKED
    | WINSPACES_STATE_SW_HIDDEN;

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

/// Whether a window on the exact-title system list is *actually on screen*,
/// and so worth hiding.
///
/// In a healthy session none of them is: the system keeps each one out of
/// sight either by never giving it `WS_VISIBLE` ("Task Host Window",
/// "Windows Push Notifications Platform" — unowned, no `WS_EX_TOOLWINDOW`, so
/// `WS_VISIBLE` alone would earn them a taskbar button) or by holding it under
/// a shell cloak ("Windows Input Experience" is `WS_VISIBLE` and 3840×2560 the
/// whole time; `DWM_CLOAKED_SHELL` is what keeps it off the screen). Either
/// way, touching it achieves nothing and risks breaking a window whose owner
/// manages its own visibility.
///
/// The one state worth acting on is a window on that list which is visible
/// *and* uncloaked by anyone — genuinely showing, with a stray taskbar button
/// to prove it. That is an anomaly, never a state to preserve, which is why
/// the reclaim pass undoes our cloak but never re-shows: WinSpaces itself used
/// to *create* this condition by handing back visibility on exit, and the next
/// scan would then read the corruption as the baseline and restore it forever.
/// Not re-showing breaks that loop and repairs it in one run.
pub(crate) fn system_window_is_showing(visible: bool, cloaked_by_anyone: bool) -> bool {
    visible && !cloaked_by_anyone
}

/// Whether dropping `hwnd` from tracking must physically restore it first.
///
/// Untracking clears the prop, and the prop is the only record that *we* hid
/// the window. Clearing it while a cloak is still applied is unrecoverable
/// from inside the daemon: the show path early-returns on `state == 0`, the
/// eligibility probe reads a cloak with no state bits as *externally* cloaked
/// (so the window never becomes valid again), and `reclaim_orphaned_windows`
/// finds nothing to reclaim at exit or startup. Only an external sweep
/// (`scripts/recover-windows.ps1`) can bring the window back.
///
/// A dead window needs nothing: there is no cloak left to undo.
pub(crate) fn must_restore_before_untrack(prev_state: usize, live: bool) -> bool {
    live && (prev_state & WINSPACES_STATE_HIDDEN_MASK) != 0
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The regression that cost the most: `SW_HIDE` is the only hiding
    /// backend that clears `WS_VISIBLE`, and it recorded no bit at all. The
    /// eligibility probe then read the window as an ordinary invisible one,
    /// so it failed `is_valid_window` permanently and the show pass skipped
    /// it — invisible, tracked, unreachable until an external sweep.
    ///
    /// Every backend must be in `HIDDEN_MASK`. Add a new one, add it here.
    #[test]
    fn every_hiding_backend_marks_the_window_hidden_by_us() {
        for (name, bit) in [
            ("DWM cloak", WINSPACES_STATE_CLOAKED),
            ("shell cloak", WINSPACES_STATE_SHELL_CLOAKED),
            ("forced minimize", WINSPACES_STATE_FORCED_MINIMIZED),
            ("SW_HIDE fallback", WINSPACES_STATE_SW_HIDDEN),
        ] {
            assert!(
                (WINSPACES_STATE_HIDDEN_MASK & bit) != 0,
                "{name} ({bit:#x}) missing from HIDDEN_MASK: a window hidden \
                 this way reads as externally hidden and is never shown again"
            );
        }
    }

    /// `SYSTEM_HIDDEN` is deliberately *not* a tracked-hide: those windows
    /// (IME, task host) are cloaked by the scan, never tracked into a space,
    /// and restored by `reclaim_orphaned_windows` on its own path.
    #[test]
    fn system_hidden_is_not_in_the_hidden_mask() {
        assert_eq!(
            WINSPACES_STATE_HIDDEN_MASK & WINSPACES_STATE_SYSTEM_HIDDEN,
            0
        );
    }

    /// The taskbar regression, pinned at its decision point. Measured states
    /// of the three windows users actually saw:
    ///
    /// | window | visible | cloaked | on screen? |
    /// |---|---|---|---|
    /// | Task Host / Push Notifications, healthy | no | no | no |
    /// | Windows Input Experience, healthy | yes | shell | no |
    /// | Task Host / Push Notifications, after a reveal | yes | no | **yes** |
    ///
    /// Only the last row is worth hiding — and it is a state WinSpaces used to
    /// create itself by re-showing these windows on exit.
    #[test]
    fn only_a_genuinely_showing_system_window_is_worth_hiding() {
        // Never visible: nothing to do, so nothing to restore later.
        assert!(!system_window_is_showing(false, false));
        // Visible but shell-cloaked (Input Experience): the system is already
        // keeping it out of sight, and its owner manages that visibility.
        assert!(!system_window_is_showing(true, true));
        // Visible and uncloaked: a real, stray taskbar button.
        assert!(system_window_is_showing(true, false));
    }

    #[test]
    fn state_bits_are_all_distinct() {
        let bits = [
            WINSPACES_STATE_TRACKED,
            WINSPACES_STATE_WAS_ICONIC,
            WINSPACES_STATE_FORCED_MINIMIZED,
            WINSPACES_STATE_CLOAKED,
            WINSPACES_STATE_SYSTEM_HIDDEN,
            WINSPACES_STATE_SHELL_CLOAKED,
            WINSPACES_STATE_SW_HIDDEN,
        ];
        for (i, a) in bits.iter().enumerate() {
            assert!(a.count_ones() == 1, "{a:#x} is not a single bit");
            for b in &bits[i + 1..] {
                assert_eq!(a & b, 0, "state bits {a:#x} and {b:#x} overlap");
            }
        }
    }

    #[test]
    fn hidden_live_window_must_be_restored_before_untracking() {
        // The regression: a window we cloaked that then fails eligibility was
        // untracked prop-and-all, stranding it invisible until an external
        // recovery sweep. Every hiding backend must be covered.
        for bit in [
            WINSPACES_STATE_CLOAKED,
            WINSPACES_STATE_SHELL_CLOAKED,
            WINSPACES_STATE_FORCED_MINIMIZED,
            WINSPACES_STATE_SW_HIDDEN,
        ] {
            assert!(
                must_restore_before_untrack(WINSPACES_STATE_TRACKED | bit, true),
                "hidden via {bit:#x} must restore before untrack"
            );
        }
    }

    #[test]
    fn visible_or_dead_windows_need_no_restore() {
        // Tracked but never hidden: nothing to undo.
        assert!(!must_restore_before_untrack(WINSPACES_STATE_TRACKED, true));
        // Dead handle: the cloak died with the window.
        assert!(!must_restore_before_untrack(
            WINSPACES_STATE_TRACKED | WINSPACES_STATE_CLOAKED,
            false
        ));
        assert!(!must_restore_before_untrack(0, true));
    }

    #[test]
    fn system_hidden_alone_is_not_a_tracked_hide() {
        // SYSTEM_HIDDEN windows (input experience, task host) are cloaked by
        // the scan, never tracked into a space, and are reclaimed by their own
        // path — they must not be dragged through the untrack restore.
        assert!(!must_restore_before_untrack(
            WINSPACES_STATE_SYSTEM_HIDDEN,
            true
        ));
    }
}
