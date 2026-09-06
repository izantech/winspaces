//! Pinned ("sticky") windows: on screen on every space of their display,
//! exempt from the hide sweep. The pin set is the only record of a pin; see
//! the field docs on `SpaceManager::sticky_windows`.

use windows_sys::Win32::Foundation::HWND;
use winspaces_common::{log_info, log_warn};

use super::eligibility::is_live_window;
use super::manager::SpaceManager;
use super::visibility::set_window_visibility;

impl SpaceManager {
    pub fn is_sticky(&self, hwnd: HWND) -> bool {
        self.sticky_windows.contains(&hwnd)
    }

    /// Whether a window tracked on `space_idx` of monitor `mon_idx` belongs on
    /// screen right now: its space is the one showing, or it is pinned.
    ///
    /// Every visibility pass must ask this rather than comparing space indices
    /// itself. The sticky exemption first landed inline in `switch_space`'s
    /// hide sweep alone, which left `reapply_visibility` (the tail of every
    /// layout restore) and `set_show_all_taskbar` cloaking pinned windows that
    /// nothing afterwards knew how to bring back — a pin is precisely an
    /// exemption from the one sweep that would have undone it.
    pub fn should_be_visible(&self, mon_idx: usize, space_idx: usize, hwnd: HWND) -> bool {
        self.monitors
            .get(mon_idx)
            .is_some_and(|m| m.current == space_idx)
            || self.is_sticky(hwnd)
    }

    /// Pin or unpin `hwnd` across every space of its display.
    ///
    /// Bookkeeping alone is not enough. The set is only consulted *by* the
    /// visibility passes, and a toggle runs none of them, so the toggle has to
    /// leave the window in the state it just promised:
    ///
    /// - Pinning something currently cloaked (its home space is not the one on
    ///   screen) shows it. From this moment the hide sweep skips it, so no
    ///   later pass would ever show it — it would sit pinned and invisible.
    /// - Unpinning one that is away from home re-homes it to the space the
    ///   user is looking at, macOS-style. The alternatives are worse: hiding a
    ///   window the user can see, or leaving it on screen but tracked to some
    ///   other space until an unrelated switch happens to sweep it away.
    ///
    /// Untracked windows are refused. The pin would otherwise live in the set
    /// while the window sat in no space list, invisible to `windows_for_space`
    /// and to every sweep — a state none of the invariants here cover.
    pub fn set_sticky(&mut self, hwnd: HWND, sticky: bool) {
        if !is_live_window(hwnd) {
            return;
        }
        let Some((mon_idx, space_idx)) = self.find_window(hwnd) else {
            log_warn!("set_sticky: hwnd {:?} is not tracked; ignoring", hwnd);
            return;
        };
        if sticky {
            if !self.sticky_windows.insert(hwnd) {
                return;
            }
            set_window_visibility(hwnd, true, self.show_all_taskbar);
            log_info!(
                "set_sticky: hwnd {:?} pinned across Mon {}'s spaces",
                hwnd,
                mon_idx + 1
            );
        } else {
            if !self.sticky_windows.remove(&hwnd) {
                return;
            }
            let current = self.monitors[mon_idx].current;
            if space_idx != current {
                self.track_window(hwnd, mon_idx, current);
            }
            log_info!(
                "set_sticky: hwnd {:?} unpinned onto Mon {}, Space {}",
                hwnd,
                mon_idx + 1,
                current + 1
            );
        }
    }

    /// Flip the pin and report what it actually became — `set_sticky` refuses
    /// untracked windows, so the caller cannot assume the flip took.
    pub fn toggle_sticky(&mut self, hwnd: HWND) -> bool {
        self.set_sticky(hwnd, !self.is_sticky(hwnd));
        self.is_sticky(hwnd)
    }
}
