//! The space-switch observer: a single `fn` pointer the bin installs so a UI
//! surface can react to a switch without this crate depending on one.
//!
//! `winspaces-core` sits *below* `winspaces-ui` and must never call into it,
//! so the direction is inverted exactly like Mission Control's `McHost`
//! vtable: the bin — the only crate that can name both sides — hands its
//! callback down as data. One choke point (`switch_space`) instead of a
//! call at every trigger site, so a future trigger cannot forget to notify.

use std::sync::OnceLock;

use windows_sys::Win32::Foundation::RECT;

/// Everything an observer needs about a completed switch, by value.
///
/// The work rect is carried rather than looked up because the observer runs
/// *inside* the caller's `AppState` borrow and must not re-enter it — the
/// bin's `with_app_state` drops re-entrant access with a warning. It is the
/// taskbar-excluded rect of the monitor that switched (`MonitorState::work`).
#[derive(Clone, Copy)]
pub struct SwitchNotice {
    pub mon_idx: usize,
    /// 0-based index of the space just activated.
    pub space_idx: usize,
    /// That monitor's total space count, for "3 of 5"-style consumers.
    pub space_count: usize,
    pub work: RECT,
}

static SWITCH_OBSERVER: OnceLock<fn(&SwitchNotice)> = OnceLock::new();

/// Install the process-wide switch observer. First call wins; later calls are
/// ignored, matching `OnceLock` semantics — the daemon installs exactly once
/// at startup.
pub fn set_switch_observer(observer: fn(&SwitchNotice)) {
    let _ = SWITCH_OBSERVER.set(observer);
}

/// Fire the installed observer, if any. Called only from `switch_space`.
pub(super) fn notify_switch(notice: &SwitchNotice) {
    if let Some(observer) = SWITCH_OBSERVER.get() {
        observer(notice);
    }
}
