//! The activation decision: a window was activated (taskbar click, app
//! activation, `SetForegroundWindow` from another process); does its monitor
//! switch to the window's space? Runs twice per activation (WinEvent +
//! ShellHook, deliberately dual), so it must stay cheap for the common case.
//!
//! Workspace rules play no part here: they are applied to windows that
//! already exist when the daemon starts or when the user asks for a restore
//! (`rules.rs`). A window that is created later belongs on the space the
//! user is looking at, so an untracked activation adopts it there.

use windows_sys::Win32::Foundation::HWND;
use windows_sys::Win32::System::SystemInformation::GetTickCount;
use windows_sys::Win32::UI::WindowsAndMessaging::{GetAncestor, GA_ROOTOWNER};
use winspaces_common::log_info;

use super::eligibility::is_valid_window;
use super::manager::SpaceManager;
use super::monitor::MonitorState;
use super::rehome::RehomeTrigger;

/// What an activation asks the daemon to do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActivationDecision {
    /// Nothing: the window is pinned, activations are suppressed, the window
    /// is not manageable, it is already on screen, or it was re-homed to the
    /// monitor it sits on.
    Ignore,
    /// Switch `mon_idx` to `space_idx` with `hwnd` activated.
    Switch {
        hwnd: HWND,
        mon_idx: usize,
        space_idx: usize,
    },
}

impl SpaceManager {
    /// Decide what the activation of `hwnd` means. Most activations are for
    /// a window already on its monitor's active space and end in a no-op, so
    /// everything up to the switch decision is answered from the tracked set
    /// in memory; the eligibility probe, with its cross-process DWM cloak
    /// query, is deferred to the switch path.
    ///
    /// `hwnd` is an opaque handle only forwarded to Win32, which tolerates a
    /// stale one by failing gracefully.
    #[allow(clippy::not_unsafe_ptr_arg_deref)]
    pub fn resolve_activation(&mut self, hwnd: HWND) -> ActivationDecision {
        // A pinned window is on screen on every space, so activating one says
        // nothing about where the user wants to be. This guard is load-bearing,
        // not defensive: `find_window` reports a window's real home space, so
        // without it, clicking a window pinned from Space 1 while standing on
        // Space 3 would drag the user back to Space 1.
        if hwnd.is_null() || self.suppress_foreground || self.is_sticky(hwnd) {
            return ActivationDecision::Ignore;
        }

        // 1. Resolve to the root owner (a child, dialog or owned popup).
        let root = unsafe { GetAncestor(hwnd, GA_ROOTOWNER) };
        if !root.is_null() && self.is_sticky(root) {
            return ActivationDecision::Ignore;
        }

        // 2. The tracked location: the root's, or the activated window's.
        let target = if !root.is_null() && root != hwnd && self.find_window(root).is_some() {
            root
        } else {
            hwnd
        };

        let (mon_idx, space_idx) = match self.find_window(target) {
            Some(loc) => loc,
            None => {
                // Not yet tracked; if valid, adopt it on the space the user
                // is on.
                if !is_valid_window(target) {
                    return ActivationDecision::Ignore;
                }
                match self.adopt_at_current(target) {
                    Some((actual_mon, cur_space)) => {
                        log_info!(
                            "Newly activated window {:?} -> tracked to Mon {}, Space {}",
                            target,
                            actual_mon + 1,
                            cur_space + 1
                        );
                        (actual_mon, cur_space)
                    }
                    None => return ActivationDecision::Ignore,
                }
            }
        };

        // 3. Moved across displays through a move that fired no MOVESIZE or
        //    foreground event (a programmatic SetWindowPos, a monitor reflow);
        //    the user's own drags are handled by the movesize hook.
        if let Some(actual_mon) = self.monitor_index_for_hwnd(target) {
            if actual_mon != mon_idx
                && self.adopt_cross_monitor_move(
                    target,
                    mon_idx,
                    actual_mon,
                    RehomeTrigger::Activation,
                )
            {
                return ActivationDecision::Ignore;
            }
        }

        // 4. The monitor's post-switch suppression window.
        let now = unsafe { GetTickCount() };
        let mon = &mut self.monitors[mon_idx];
        if !foreground_suppression_elapsed(mon, now) {
            return ActivationDecision::Ignore;
        }

        // 5. Already on the active space of that monitor: nothing to switch.
        if mon.current == space_idx {
            return ActivationDecision::Ignore;
        }

        // 6. A switch is about to happen: now the eligibility probe is worth
        //    its cost. A tracked but externally-cloaked window must not
        //    trigger one.
        if !is_valid_window(target) {
            return ActivationDecision::Ignore;
        }

        log_info!(
            "Window activation for {:?} -> Switching Display {} from Space {} to Space {}",
            target,
            mon_idx + 1,
            mon.current + 1,
            space_idx + 1
        );
        ActivationDecision::Switch {
            hwnd: target,
            mon_idx,
            space_idx,
        }
    }
}

/// Whether `mon`'s post-switch foreground suppression has elapsed at `now`
/// (`GetTickCount` units, wrap-safe); clears the deadline once it has.
pub(crate) fn foreground_suppression_elapsed(mon: &mut MonitorState, now: u32) -> bool {
    if mon.suppress_foreground_until == 0 {
        return true;
    }
    if (now as i32).wrapping_sub(mon.suppress_foreground_until as i32) < 0 {
        return false;
    }
    mon.suppress_foreground_until = 0;
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    const WINDOW: HWND = 100 as HWND;

    #[test]
    fn suppression_elapses_and_clears_with_tick_wraparound() {
        let mut mon = MonitorState::for_test(0, vec![vec![]]);
        assert!(foreground_suppression_elapsed(&mut mon, 1000));

        mon.suppress_foreground_until = 5000;
        assert!(!foreground_suppression_elapsed(&mut mon, 4999));
        assert_eq!(mon.suppress_foreground_until, 5000, "not cleared early");
        assert!(foreground_suppression_elapsed(&mut mon, 5000));
        assert_eq!(mon.suppress_foreground_until, 0, "cleared once elapsed");

        // A deadline set just before the 49.7-day tick wrap.
        mon.suppress_foreground_until = u32::MAX - 10;
        assert!(!foreground_suppression_elapsed(&mut mon, u32::MAX - 20));
        assert!(foreground_suppression_elapsed(&mut mon, 5));
    }

    #[test]
    fn a_pinned_or_suppressed_activation_is_ignored_before_any_probe() {
        let mut mgr = SpaceManager::for_test(vec![MonitorState::for_test(0, vec![vec![WINDOW]])]);
        mgr.sticky_windows.insert(WINDOW);
        assert_eq!(mgr.resolve_activation(WINDOW), ActivationDecision::Ignore);

        let mut mgr = SpaceManager::for_test(vec![MonitorState::for_test(0, vec![vec![WINDOW]])]);
        mgr.suppress_foreground = true;
        assert_eq!(mgr.resolve_activation(WINDOW), ActivationDecision::Ignore);
        assert_eq!(
            mgr.resolve_activation(std::ptr::null_mut()),
            ActivationDecision::Ignore
        );
    }

    #[test]
    fn an_untracked_dead_handle_is_ignored() {
        let mut mgr = SpaceManager::for_test(vec![MonitorState::for_test(0, vec![vec![]])]);
        assert_eq!(mgr.resolve_activation(WINDOW), ActivationDecision::Ignore);
        assert_eq!(
            mgr.find_window(WINDOW),
            None,
            "a dead handle is never adopted"
        );
    }
}
