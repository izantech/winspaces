use crate::{log_info, log_warn};
use std::collections::HashMap;
use std::ffi::c_void;
use std::ptr::{null, null_mut};
use windows_sys::Win32::Foundation::{BOOL, HWND, LPARAM, POINT, RECT};
use windows_sys::Win32::Graphics::Dwm::{
    DwmGetWindowAttribute, DwmSetWindowAttribute, DWMWA_CLOAK, DWMWA_CLOAKED,
};
use windows_sys::Win32::Graphics::Gdi::{
    EnumDisplayMonitors, GetMonitorInfoW, MonitorFromPoint, MonitorFromWindow, HDC, HMONITOR,
    MONITORINFOEXW, MONITOR_DEFAULTTONEAREST,
};
use windows_sys::Win32::System::SystemInformation::GetTickCount;
use windows_sys::Win32::UI::HiDpi::{GetDpiForMonitor, MDT_EFFECTIVE_DPI};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::SetActiveWindow;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GetAncestor, GetClassNameW, GetCursorPos, GetForegroundWindow, GetPropA,
    GetWindowLongW, GetWindowRect, GetWindowTextW, GetWindowThreadProcessId, IsIconic, IsWindow,
    RemovePropA, SetForegroundWindow, SetPropA, SetWindowPos, ShowWindow, SystemParametersInfoW,
    ANIMATIONINFO, GA_ROOTOWNER, GWL_EXSTYLE, GWL_STYLE, SPI_GETANIMATION, SPI_SETANIMATION,
    SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SWP_NOZORDER, SWP_SHOWWINDOW, SW_FORCEMINIMIZE,
    SW_HIDE, SW_SHOWMINNOACTIVE, SW_SHOWNA, SW_SHOWNOACTIVATE, WS_EX_APPWINDOW, WS_EX_NOACTIVATE,
    WS_EX_TOOLWINDOW, WS_VISIBLE,
};
use winspaces_common::{DEFAULT_DESKTOPS, MAX_DESKTOPS};

pub const MAX_MONITORS: usize = 8;
const WINSPACES_PROP_STATE: &[u8] = b"WinSpacesWindowState\0";
/// How long a completed scan exempts `switch_desktop` from running another
/// one. Rapid space-stepping would otherwise pay a full `EnumWindows` (with a
/// cross-process DWM probe per window) on every hop.
const SCAN_THROTTLE_MS: u32 = 250;

const WINSPACES_STATE_TRACKED: usize = 0x01;
const WINSPACES_STATE_WAS_ICONIC: usize = 0x02;
const WINSPACES_STATE_FORCED_MINIMIZED: usize = 0x04;
const WINSPACES_STATE_CLOAKED: usize = 0x08;
// System windows (input experience, task host, ...) that we cloaked out of the
// way. Marked so exit/startup passes can undo the cloak: DWM cloaks persist
// after the process that applied them dies.
const WINSPACES_STATE_SYSTEM_HIDDEN: usize = 0x10;
// Hidden via the ImmersiveShell cloak (`shell_cloak.rs`). A DWM uncloak does
// NOT clear this kind of cloak — recovery must go through the same COM call.
const WINSPACES_STATE_SHELL_CLOAKED: usize = 0x20;

/// Every state bit that means "we hid this window". Shared by the eligibility
/// probe, the scan skip, crash recovery, and the hide early-return so a new
/// hiding backend cannot be forgotten in one of them.
const WINSPACES_STATE_HIDDEN_MASK: usize =
    WINSPACES_STATE_CLOAKED | WINSPACES_STATE_FORCED_MINIMIZED | WINSPACES_STATE_SHELL_CLOAKED;

/// Facts about a window that the eligibility decision needs, gathered from
/// Win32 by `is_valid_window` so the decision itself (`is_eligible`) stays
/// pure and unit-testable.
struct WindowFacts {
    style: u32,
    ex_style: u32,
    class_name: String,
    has_title: bool,
    /// DWM-cloaked without our CLOAKED state bit: cloaked by Windows or
    /// another app (suspended UWP, native virtual desktops, ...).
    externally_cloaked: bool,
    /// Our state bits say we hid it (cloak or forced minimize), so a clear
    /// WS_VISIBLE / set cloak must not disqualify it.
    hidden_by_us: bool,
    /// Root-owner facts; None when unowned or when WS_EX_APPWINDOW asks to
    /// be judged standalone.
    owner: Option<Box<WindowFacts>>,
}

/// Shell/system window classes that are never managed. Classes are stable
/// identifiers across locales and Windows builds, unlike window titles.
fn is_shell_class(class_name: &str) -> bool {
    matches!(
        class_name,
        "Progman"
            | "WorkerW"
            | "Shell_TrayWnd"
            | "Shell_SecondaryTrayWnd"
            | "Windows.UI.Core.CoreWindow"
            | "EdgeUiInputTopWndClass"
            | "XamlExplorerHost"
            | "TopLevelWindowForOverflowXamlIsland"
            | "Windows.UI.Composition.DesktopWindowTarget"
            | "TaskListThumbnailWnd"
            | "NativeHWNDHost"
            | "PopupHost"
            | "Xaml_WindowedPopupClass"
            | "IME"
            | "MSCTFIME UI"
            | "tooltips_class32"
            | "SysShadow"
            | "ComboLBox"
            | "#32768"
    )
}

/// Structural noise checks shared by a window and its root owner, modeled on
/// the shell's Alt-Tab rules (visibility, extended styles, cloak state)
/// instead of title blacklists. `require_title` is off for the owner: only
/// the window itself must be titled.
fn passes_structural_checks(facts: &WindowFacts, require_title: bool) -> bool {
    if (facts.ex_style & WS_EX_TOOLWINDOW) != 0 {
        return false;
    }
    // Never-activated windows (overlays, OSDs) don't appear on the taskbar
    // unless WS_EX_APPWINDOW forces them in.
    if (facts.ex_style & WS_EX_NOACTIVATE) != 0 && (facts.ex_style & WS_EX_APPWINDOW) == 0 {
        return false;
    }
    if is_shell_class(&facts.class_name) {
        return false;
    }
    if require_title && !facts.has_title {
        return false;
    }
    if facts.externally_cloaked {
        return false;
    }
    if (facts.style & WS_VISIBLE) == 0 && !facts.hidden_by_us {
        return false;
    }
    true
}

/// A window is manageable when it passes the structural checks itself and,
/// if owned, its root owner does too. Owned dialogs of real apps stay
/// individually managed (they must cloak with their app on space switches);
/// popups of hidden or tool-window owners are noise.
fn is_eligible(facts: &WindowFacts) -> bool {
    if !passes_structural_checks(facts, true) {
        return false;
    }
    match &facts.owner {
        Some(owner) => passes_structural_checks(owner, false),
        None => true,
    }
}

unsafe fn gather_window_facts(hwnd: HWND, follow_owner: bool) -> WindowFacts {
    let style = GetWindowLongW(hwnd, GWL_STYLE) as u32;
    let ex_style = GetWindowLongW(hwnd, GWL_EXSTYLE) as u32;

    let mut class_buf = [0u16; 256];
    let clen = GetClassNameW(hwnd, class_buf.as_mut_ptr(), class_buf.len() as i32);
    let class_name = if clen > 0 {
        String::from_utf16_lossy(&class_buf[..clen as usize])
    } else {
        String::new()
    };

    let mut title = [0u16; 2];
    let has_title = GetWindowTextW(hwnd, title.as_mut_ptr(), title.len() as i32) > 0;

    let state = get_window_state(hwnd);
    let mut cloaked: u32 = 0;
    let hr = DwmGetWindowAttribute(
        hwnd,
        DWMWA_CLOAKED as _,
        &mut cloaked as *mut _ as _,
        std::mem::size_of::<u32>() as u32,
    );
    let externally_cloaked = hr == 0
        && cloaked != 0
        && (state & (WINSPACES_STATE_CLOAKED | WINSPACES_STATE_SHELL_CLOAKED)) == 0;
    let hidden_by_us = (state & WINSPACES_STATE_HIDDEN_MASK) != 0;

    let owner = if follow_owner && (ex_style & WS_EX_APPWINDOW) == 0 {
        let root = GetAncestor(hwnd, GA_ROOTOWNER);
        if !root.is_null() && root != hwnd {
            Some(Box::new(gather_window_facts(root, false)))
        } else {
            None
        }
    } else {
        None
    };

    WindowFacts {
        style,
        ex_style,
        class_name,
        has_title,
        externally_cloaked,
        hidden_by_us,
        owner,
    }
}

pub fn is_valid_window(hwnd: HWND) -> bool {
    unsafe {
        if hwnd.is_null() || IsWindow(hwnd) == 0 {
            return false;
        }

        let mut pid: u32 = 0;
        GetWindowThreadProcessId(hwnd, &mut pid);
        if pid == 0 || pid == std::process::id() {
            return false;
        }

        is_eligible(&gather_window_facts(hwnd, true))
    }
}

fn get_window_state(hwnd: HWND) -> usize {
    unsafe { GetPropA(hwnd, WINSPACES_PROP_STATE.as_ptr()) as usize }
}

fn set_window_state(hwnd: HWND, state: usize) {
    unsafe {
        if state == 0 {
            RemovePropA(hwnd, WINSPACES_PROP_STATE.as_ptr());
        } else {
            SetPropA(hwnd, WINSPACES_PROP_STATE.as_ptr(), state as *mut c_void);
        }
    }
}

pub struct MonitorState {
    pub hmon: HMONITOR,
    /// GDI device name (`\\.\DISPLAY1`, ...). A *slot* name that Windows
    /// recycles by attach order — kept for logging, never used as an identity.
    pub device: String,
    /// Monitor device path from `QueryDisplayConfig`, e.g.
    /// `\\?\DISPLAY#BNQ805B#5&1f33c64f&0&UID4356#{...}`. This is the identity
    /// that survives RDP, docking and re-plugging. Falls back to `device` when
    /// the display-config API is unavailable.
    pub stable_id: String,
    /// Full monitor bounds in physical pixels, refreshed on enumeration. Lets
    /// a window be resolved to a monitor by geometry when its cached HMONITOR
    /// has gone stale.
    pub rect: RECT,
    pub work: RECT,
    pub current: usize,
    pub last_switched_desk: usize,
    pub last_switch_time: u32,
    pub suppress_foreground_until: u32,
    /// One entry per space; `desktops.len()` IS this monitor's space count
    /// (always in `1..=MAX_DESKTOPS`). Counts are per monitor, so every bounds
    /// check must go through this length, never a global constant.
    pub desktops: Vec<Vec<HWND>>,
}

/// Wrapping-safe "`now` has not yet reached `deadline`". `GetTickCount` rolls
/// over every ~49 days, so a plain `<` breaks once per rollover. A zero
/// deadline means "no deadline set" — without that guard, any machine up for
/// more than 24.8 days would read as permanently settling.
fn tick_before(now: u32, deadline: u32) -> bool {
    deadline != 0 && now.wrapping_sub(deadline) > u32::MAX / 2
}

/// Space that inherits the windows of a removed space, in the indexing that
/// is live *while the removed space still exists* (`track_window` runs before
/// the `Vec::remove`). macOS semantics: occupants go to the space on the
/// left; the first space has no left neighbour, so its windows fall right
/// onto old space 1 — which becomes space 0 once the removal shifts.
fn removal_migration_target(removed: usize) -> usize {
    if removed == 0 {
        1
    } else {
        removed - 1
    }
}

/// Where a stored space index points after space `removed` has been deleted.
/// An index *on* the removed space follows its migrated windows left.
fn remap_index_after_removal(idx: usize, removed: usize) -> usize {
    if idx > removed {
        idx - 1
    } else if idx == removed {
        idx.saturating_sub(1)
    } else {
        idx
    }
}

/// `(szDevice, rcMonitor, rcWork)` for a monitor handle.
fn monitor_geometry(hmon: HMONITOR) -> (String, RECT, RECT) {
    unsafe {
        let mut mi: MONITORINFOEXW = std::mem::zeroed();
        mi.monitorInfo.cbSize = std::mem::size_of::<MONITORINFOEXW>() as u32;
        if GetMonitorInfoW(hmon, &mut mi as *mut _ as *mut _) != 0 {
            (
                crate::topology::wide_to_string(&mi.szDevice),
                mi.monitorInfo.rcMonitor,
                mi.monitorInfo.rcWork,
            )
        } else {
            (String::new(), std::mem::zeroed(), std::mem::zeroed())
        }
    }
}

impl MonitorState {
    pub fn new(hmon: HMONITOR, stable_ids: &HashMap<String, String>) -> Self {
        let (device, rect, work) = monitor_geometry(hmon);
        let stable_id = stable_ids
            .get(&device)
            .cloned()
            .unwrap_or_else(|| device.clone());
        Self {
            hmon,
            device,
            stable_id,
            rect,
            work,
            current: 0,
            last_switched_desk: 0,
            last_switch_time: 0,
            suppress_foreground_until: 0,
            desktops: vec![Vec::new(); DEFAULT_DESKTOPS],
        }
    }

    fn contains(&self, pt: POINT) -> bool {
        pt.x >= self.rect.left
            && pt.x < self.rect.right
            && pt.y >= self.rect.top
            && pt.y < self.rect.bottom
    }

    /// Effective DPI, used to decide whether a stored rect can be replayed
    /// pixel-for-pixel or has to be reprojected.
    pub fn dpi(&self) -> u32 {
        unsafe {
            let mut x: u32 = 96;
            let mut y: u32 = 96;
            if GetDpiForMonitor(self.hmon, MDT_EFFECTIVE_DPI, &mut x, &mut y) == 0 {
                x
            } else {
                96
            }
        }
    }
}

pub struct DesktopManager {
    pub monitors: Vec<MonitorState>,
    pub handle_hotkeys: bool,
    pub show_all_taskbar: bool,
    pub suppress_foreground: bool,
    /// Set between the first `WM_DISPLAYCHANGE` of a burst and the debounced
    /// reconcile that follows. While set, scans stop re-homing windows across
    /// monitors — the OS is mid-reflow and any conclusion drawn now is wrong.
    pub reconcile_pending: bool,
    /// Tick deadline after a bulk restore, during which cross-monitor re-homing
    /// is suppressed. `SetWindowPlacement`/`SetWindowPos` do not take effect
    /// synchronously — a scan moments later still sees the window at its old
    /// coordinates and "corrects" the tracking to match, dumping it on that
    /// monitor's current space. Observed 90 ms after a restore.
    pub suppress_rehome_until: u32,
    /// Tick of the last completed `scan_untracked_windows`, for the
    /// `SCAN_THROTTLE_MS` exemption in `switch_desktop`.
    last_scan_tick: u32,
}

impl DesktopManager {
    pub fn new() -> Self {
        // A previous instance may have died (crash, taskkill) leaving windows
        // cloaked/minimized with our state props still attached. Restore them
        // before scanning, otherwise they stay invisible forever.
        reclaim_orphaned_windows();
        let mut mgr = Self {
            monitors: Vec::new(),
            handle_hotkeys: true,
            show_all_taskbar: true,
            suppress_foreground: false,
            reconcile_pending: false,
            suppress_rehome_until: 0,
            last_scan_tick: 0,
        };
        mgr.update_monitors();
        mgr.scan_untracked_windows();
        mgr
    }

    pub fn set_show_all_taskbar(&mut self, new_val: bool) {
        if self.show_all_taskbar == new_val {
            return;
        }
        log_info!(
            "Changing show_all_taskbar mode from {} to {}",
            self.show_all_taskbar,
            new_val
        );

        let old_val = self.show_all_taskbar;
        self.show_all_taskbar = new_val;

        // Re-hide windows on inactive spaces in the new mode: show with the old
        // mode first so the hide path doesn't early-return on the already-set
        // CLOAKED/FORCED_MINIMIZED state bits.
        for m_idx in 0..self.monitors.len() {
            let current = self.monitors[m_idx].current;
            for d_idx in 0..self.monitors[m_idx].desktops.len() {
                if d_idx == current {
                    continue;
                }
                let windows = self.monitors[m_idx].desktops[d_idx].clone();
                for &hwnd in &windows {
                    if is_valid_window(hwnd) {
                        set_window_visibility(hwnd, true, old_val);
                        set_window_visibility(hwnd, false, new_val);
                    }
                }
            }
        }
    }

    pub fn update_monitors(&mut self) {
        self.monitors.clear();
        // One display-config query for the whole enumeration: it walks every
        // active path, so doing it per monitor would be quadratic.
        let mut ctx = EnumMonitorsContext {
            monitors: Vec::new(),
            stable_ids: crate::topology::stable_monitor_ids(),
        };
        unsafe {
            EnumDisplayMonitors(
                null_mut(),
                null(),
                Some(enum_monitors_callback),
                &mut ctx as *mut _ as LPARAM,
            );
        }
        self.monitors = ctx.monitors;
    }

    /// Order-independent identity of the attached monitor set.
    pub fn topology_signature(&self) -> String {
        let ids: Vec<String> = self.monitors.iter().map(|m| m.stable_id.clone()).collect();
        crate::topology::signature_from_ids(&ids)
    }

    /// Resolve the monitor a window sits on. Prefers the `HMONITOR` identity,
    /// then falls back to locating the window's centre inside a monitor's
    /// bounds.
    ///
    /// The fallback matters: `MonitorFromWindow` can hand back a handle that is
    /// not in our table (reissued after a topology change, or a monitor beyond
    /// `MAX_MONITORS`). Callers previously treated that as "monitor 0", which
    /// silently dragged every window onto the primary display and onto that
    /// display's *current* space — destroying the user's space assignments with
    /// no display change involved. Returning `None` lets callers leave the
    /// window's tracking alone instead of corrupting it.
    pub fn monitor_index_for_hwnd(&self, hwnd: HWND) -> Option<usize> {
        unsafe {
            let hmon = MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST);
            if let Some(idx) = self.monitors.iter().position(|m| m.hmon == hmon) {
                return Some(idx);
            }

            let mut r: RECT = std::mem::zeroed();
            if GetWindowRect(hwnd, &mut r) == 0 {
                return None;
            }
            let centre = POINT {
                x: r.left + (r.right - r.left) / 2,
                y: r.top + (r.bottom - r.top) / 2,
            };
            self.monitors.iter().position(|m| m.contains(centre))
        }
    }

    /// Rebuild the monitor list after WM_DISPLAYCHANGE, carrying per-monitor
    /// space state over by stable device path (HMONITORs may be reissued, and
    /// `\\.\DISPLAYn` slot names get recycled onto entirely different physical
    /// monitors). Windows tracked on a vanished monitor are made visible and
    /// re-scanned onto whichever monitor the OS moved them to.
    pub fn handle_display_change(&mut self) {
        let old_monitors = std::mem::take(&mut self.monitors);
        self.update_monitors();
        log_info!(
            "Display change: {} -> {} monitors [{}]",
            old_monitors.len(),
            self.monitors.len(),
            self.monitors
                .iter()
                .map(|m| format!("{} {}", m.device, m.stable_id))
                .collect::<Vec<_>>()
                .join(", ")
        );

        let show_all = self.show_all_taskbar;
        for old in old_monitors {
            let new_idx = self
                .monitors
                .iter()
                .position(|m| m.stable_id == old.stable_id && !old.stable_id.is_empty());
            match new_idx {
                Some(idx) => {
                    self.monitors[idx].current = old.current;
                    self.monitors[idx].last_switched_desk = old.last_switched_desk;
                    self.monitors[idx].last_switch_time = old.last_switch_time;
                    self.monitors[idx].desktops = old.desktops;
                }
                None => {
                    for desk in &old.desktops {
                        for &hwnd in desk {
                            if is_valid_window(hwnd) {
                                set_window_visibility(hwnd, true, show_all);
                            }
                            set_window_state(hwnd, 0);
                        }
                    }
                }
            }
        }
        self.scan_untracked_windows();
    }

    /// Hold off cross-monitor re-homing for `ms`. Call after any bulk
    /// placement: the scan must not treat not-yet-applied geometry as the user
    /// having dragged the window to another display.
    pub fn begin_settle(&mut self, ms: u32) {
        self.suppress_rehome_until = unsafe { GetTickCount() }.wrapping_add(ms);
    }

    pub fn is_settling(&self) -> bool {
        tick_before(unsafe { GetTickCount() }, self.suppress_rehome_until)
    }

    /// Show every window on each monitor's current space and hide the rest.
    /// Used after a bulk re-track (snapshot replay) where per-monitor `current`
    /// was set directly rather than by walking `switch_desktop`.
    pub fn reapply_visibility(&mut self) {
        let show_all = self.show_all_taskbar;
        let _no_anim = show_all.then(AnimationGuard::new);
        for mon in &self.monitors {
            for (d_idx, desk) in mon.desktops.iter().enumerate() {
                for &hwnd in desk {
                    if is_valid_window(hwnd) {
                        set_window_visibility(hwnd, d_idx == mon.current, show_all);
                    }
                }
            }
        }
    }

    /// Make every tracked window visible without disturbing space membership.
    /// The replay path needs this before moving windows: geometry applied to a
    /// cloaked or force-minimized window does not stick.
    pub fn show_all_tracked(&mut self) {
        let show_all = self.show_all_taskbar;
        for mon in &self.monitors {
            for desk in mon.desktops.iter() {
                for &hwnd in desk {
                    if is_valid_window(hwnd) {
                        set_window_visibility(hwnd, true, show_all);
                    }
                }
            }
        }
    }

    pub fn get_active_monitor_index(&self) -> usize {
        if self.monitors.is_empty() {
            return 0;
        }
        unsafe {
            let mut pt = POINT { x: 0, y: 0 };
            GetCursorPos(&mut pt);
            let hmon = MonitorFromPoint(pt, MONITOR_DEFAULTTONEAREST);
            for (idx, mon) in self.monitors.iter().enumerate() {
                if mon.hmon == hmon {
                    return idx;
                }
            }

            let fg = GetForegroundWindow();
            if !fg.is_null() {
                let hmon_fg = MonitorFromWindow(fg, MONITOR_DEFAULTTONEAREST);
                for (idx, mon) in self.monitors.iter().enumerate() {
                    if mon.hmon == hmon_fg {
                        return idx;
                    }
                }
            }
        }
        0
    }

    pub fn find_window(&self, hwnd: HWND) -> Option<(usize, usize)> {
        for (m_idx, mon) in self.monitors.iter().enumerate() {
            for (d_idx, desk) in mon.desktops.iter().enumerate() {
                if desk.contains(&hwnd) {
                    return Some((m_idx, d_idx));
                }
            }
        }
        None
    }

    fn remove_window(&mut self, hwnd: HWND) -> bool {
        let mut removed = false;
        for mon in &mut self.monitors {
            for desk in &mut mon.desktops {
                if let Some(pos) = desk.iter().position(|&h| h == hwnd) {
                    desk.remove(pos);
                    removed = true;
                }
            }
        }
        if removed {
            set_window_state(hwnd, 0);
        }
        removed
    }

    pub fn track_window(&mut self, hwnd: HWND, mon_idx: usize, desk_idx: usize) {
        self.remove_window(hwnd);

        if self.monitors.is_empty() || !is_valid_window(hwnd) {
            return;
        }

        // Workspace rules and IPC callers may reference a display that is not
        // currently attached (undocked laptop, powered-off screen). Fall back
        // to the window's actual monitor instead of indexing out of bounds —
        // this used to abort the daemon during startup rule restore. If the
        // window cannot be resolved either, leave it untracked rather than
        // dumping it on the primary display.
        let mon_idx = if mon_idx < self.monitors.len() {
            mon_idx
        } else {
            match self.monitor_index_for_hwnd(hwnd) {
                Some(fallback) => {
                    log_info!(
                        "track_window: Display {} not attached; falling back to Mon {}",
                        mon_idx + 1,
                        fallback + 1
                    );
                    fallback
                }
                None => {
                    log_warn!(
                        "track_window: Display {} not attached and hwnd {:?} resolves to no monitor; leaving untracked",
                        mon_idx + 1,
                        hwnd
                    );
                    return;
                }
            }
        };
        let desk_idx = desk_idx.min(self.monitors[mon_idx].desktops.len() - 1);

        let mut state = WINSPACES_STATE_TRACKED;
        unsafe {
            if IsIconic(hwnd) != 0 {
                state |= WINSPACES_STATE_WAS_ICONIC;
            }
        }
        set_window_state(hwnd, state);
        self.monitors[mon_idx].desktops[desk_idx].push(hwnd);
        log_info!(
            "track_window: hwnd {:?} -> Mon {}, Desk {}",
            hwnd,
            mon_idx + 1,
            desk_idx + 1
        );
    }

    pub fn scan_untracked_windows(&mut self) {
        struct ScanContext<'a> {
            mgr: &'a mut DesktopManager,
        }

        unsafe extern "system" fn enum_windows_proc(hwnd: HWND, lparam: LPARAM) -> BOOL {
            let ctx = &mut *(lparam as *mut ScanContext);

            let mut title = [0u16; 256];
            let len = GetWindowTextW(hwnd, title.as_mut_ptr(), title.len() as i32);
            if len > 0 {
                let title_str = String::from_utf16_lossy(&title[..len as usize]);
                let lower_title = title_str.to_lowercase();
                // Exact titles only: a substring match here would cloak
                // legitimate user windows (e.g. a browser tab mentioning
                // "input experience").
                if lower_title == "windows input experience"
                    || lower_title == "experiencia de entrada de windows"
                    || lower_title == "task host window"
                    || lower_title == "windows push notifications platform"
                    || lower_title == "coremessaging"
                    || lower_title == "default ime"
                    || lower_title == "msctfime ui"
                    || lower_title == "popuphost"
                {
                    let state = get_window_state(hwnd);
                    if (state & WINSPACES_STATE_SYSTEM_HIDDEN) == 0 {
                        let mut one: i32 = 1;
                        DwmSetWindowAttribute(
                            hwnd,
                            DWMWA_CLOAK as _,
                            &mut one as *mut _ as _,
                            std::mem::size_of::<i32>() as u32,
                        );
                        ShowWindow(hwnd, SW_HIDE);
                        // Mark it so exit/startup passes can undo the cloak.
                        set_window_state(hwnd, state | WINSPACES_STATE_SYSTEM_HIDDEN);
                    }
                    return 1;
                }
            }

            let state = get_window_state(hwnd);
            if (state & WINSPACES_STATE_HIDDEN_MASK) != 0 {
                return 1;
            }

            if is_valid_window(hwnd) {
                // No resolvable monitor means a stale HMONITOR mid-topology
                // change. Leaving the window where it is beats claiming it for
                // the primary display.
                let Some(actual_mon_idx) = ctx.mgr.monitor_index_for_hwnd(hwnd) else {
                    log_warn!(
                        "scan: hwnd {:?} resolves to no monitor; leaving tracking unchanged",
                        hwnd
                    );
                    return 1;
                };

                match ctx.mgr.find_window(hwnd) {
                    None => {
                        let desk_idx = ctx.mgr.monitors[actual_mon_idx].current;
                        ctx.mgr.track_window(hwnd, actual_mon_idx, desk_idx);
                    }
                    Some((curr_mon, _curr_desk)) if curr_mon != actual_mon_idx => {
                        // Re-homing moves the window onto the target monitor's
                        // *current* space, which is correct for a user drag but
                        // destroys space assignments wholesale when it fires
                        // during a topology change or right after a restore —
                        // in both cases the window is somewhere transient, not
                        // somewhere the user put it.
                        if ctx.mgr.reconcile_pending || ctx.mgr.is_settling() {
                            return 1;
                        }
                        let desk_idx = ctx.mgr.monitors[actual_mon_idx].current;
                        log_info!(
                            "Window {:?} moved across displays from Mon {} to Mon {} (Space {})",
                            hwnd,
                            curr_mon + 1,
                            actual_mon_idx + 1,
                            desk_idx + 1
                        );
                        ctx.mgr.track_window(hwnd, actual_mon_idx, desk_idx);
                    }
                    _ => {}
                }
            }
            1
        }

        let mut ctx = ScanContext { mgr: self };
        unsafe {
            EnumWindows(Some(enum_windows_proc), &mut ctx as *mut _ as LPARAM);
        }
        self.last_scan_tick = unsafe { GetTickCount() };
    }

    pub fn go_to_desk(&mut self, target_desk: usize) {
        if self.monitors.is_empty() {
            return;
        }
        let mon_idx = self.get_active_monitor_index();
        // Bounds are per monitor: Alt+7 with the cursor on a 4-space monitor
        // is a deliberate no-op, not a clamp.
        if target_desk >= self.monitors[mon_idx].desktops.len() {
            return;
        }

        if self.monitors[mon_idx].current == target_desk {
            return;
        }

        self.switch_desktop(mon_idx, target_desk, None);
    }

    pub fn step_desktop(&mut self, delta: i32) {
        if self.monitors.is_empty() {
            return;
        }
        let mon_idx = self.get_active_monitor_index();
        let cur = self.monitors[mon_idx].current as i32;
        let num = self.monitors[mon_idx].desktops.len() as i32;
        let next = (cur + delta).rem_euclid(num) as usize;
        self.go_to_desk(next);
    }

    pub fn move_to_desk(&mut self, target_desk: usize) {
        if self.monitors.is_empty() {
            return;
        }
        let fg = unsafe { GetForegroundWindow() };
        if fg.is_null() || !is_valid_window(fg) {
            return;
        }

        let mon_idx = self.get_active_monitor_index();
        if target_desk >= self.monitors[mon_idx].desktops.len() {
            return;
        }
        let cur = self.monitors[mon_idx].current;
        if cur == target_desk {
            return;
        }

        self.track_window(fg, mon_idx, target_desk);
        self.switch_desktop(mon_idx, target_desk, Some(fg));
    }

    pub fn step_move_window(&mut self, delta: i32) {
        if self.monitors.is_empty() {
            return;
        }
        let mon_idx = self.get_active_monitor_index();
        let cur = self.monitors[mon_idx].current as i32;
        let num = self.monitors[mon_idx].desktops.len() as i32;
        let next = (cur + delta).rem_euclid(num) as usize;
        self.move_to_desk(next);
    }

    pub fn switch_desktop(
        &mut self,
        mon_idx: usize,
        target_desk: usize,
        activate_window: Option<HWND>,
    ) {
        if mon_idx >= self.monitors.len() || target_desk >= self.monitors[mon_idx].desktops.len() {
            return;
        }

        let now = unsafe { GetTickCount() };
        if !tick_before(now, self.last_scan_tick.wrapping_add(SCAN_THROTTLE_MS)) {
            self.scan_untracked_windows();
        }

        let old_desk = self.monitors[mon_idx].current;

        log_info!(
            "switch_desktop: Mon {} from Space {} to Space {}",
            mon_idx + 1,
            old_desk + 1,
            target_desk + 1
        );

        self.monitors[mon_idx].last_switched_desk = old_desk;
        self.monitors[mon_idx].last_switch_time = now;
        self.monitors[mon_idx].current = target_desk;

        // Minimizing the old foreground makes the OS activate some other
        // window, and that event arrives asynchronously after this function
        // returns. Arm the guard on every monitor, not just the switching
        // one — the echo can land on a window tracked elsewhere and trigger
        // a phantom switch there.
        for mon in &mut self.monitors {
            mon.suppress_foreground_until = now.wrapping_add(500);
        }

        let show_all = self.show_all_taskbar;
        // The cloak path has no OS animation to suppress.
        let _no_anim = show_all.then(AnimationGuard::new);

        // Show the incoming space first, then drop the outgoing one: the
        // shows don't activate, so the new windows surface beneath the old
        // ones for a few frames instead of the desktop showing through.
        let target_windows = self.monitors[mon_idx].desktops[target_desk].clone();
        for &hwnd in &target_windows {
            if is_valid_window(hwnd) {
                set_window_visibility(hwnd, true, show_all);
            }
        }

        for d_idx in 0..self.monitors[mon_idx].desktops.len() {
            if d_idx == target_desk {
                continue;
            }
            let windows = self.monitors[mon_idx].desktops[d_idx].clone();
            for &hwnd in &windows {
                if is_valid_window(hwnd) {
                    set_window_visibility(hwnd, false, show_all);
                }
            }
        }

        if let Some(act_hwnd) = activate_window {
            unsafe {
                SetForegroundWindow(act_hwnd);
                SetActiveWindow(act_hwnd);
            }
        } else {
            let mut activated = false;
            for &hwnd in target_windows.iter().rev() {
                if is_valid_window(hwnd) {
                    let state = get_window_state(hwnd);
                    if (state & WINSPACES_STATE_WAS_ICONIC) == 0 {
                        unsafe {
                            SetForegroundWindow(hwnd);
                        }
                        activated = true;
                        break;
                    }
                }
            }

            if !activated {
                if let Some(&first_hwnd) = target_windows.iter().next_back() {
                    if is_valid_window(first_hwnd) {
                        unsafe {
                            SetForegroundWindow(first_hwnd);
                        }
                    }
                }
            }
        }
    }

    /// Append an empty space to a monitor. Does not switch to it (macOS
    /// doesn't either). Returns whether anything changed.
    pub fn add_space(&mut self, mon_idx: usize) -> bool {
        if mon_idx >= self.monitors.len() {
            return false;
        }
        if self.monitors[mon_idx].desktops.len() >= MAX_DESKTOPS {
            return false;
        }
        self.monitors[mon_idx].desktops.push(Vec::new());
        log_info!(
            "add_space: Mon {} now has {} spaces",
            mon_idx + 1,
            self.monitors[mon_idx].desktops.len()
        );
        true
    }

    /// Remove space `desk_idx` from a monitor, migrating its windows to the
    /// space on the left (macOS semantics). Returns whether anything changed.
    ///
    /// Membership moves are pure Vec+prop operations via `track_window`; the
    /// single `switch_desktop` at the end is what resolves visibility — it
    /// re-shows the (possibly unchanged) current space and hides every other,
    /// which covers both "removed the current space" and "removed a background
    /// space whose windows migrated onto the current one".
    pub fn remove_space(&mut self, mon_idx: usize, desk_idx: usize) -> bool {
        if mon_idx >= self.monitors.len() {
            return false;
        }
        let len = self.monitors[mon_idx].desktops.len();
        if len <= 1 || desk_idx >= len {
            return false;
        }

        let occupants = self.monitors[mon_idx].desktops[desk_idx].clone();
        let target = removal_migration_target(desk_idx);
        for &hwnd in &occupants {
            self.track_window(hwnd, mon_idx, target);
        }

        let mon = &mut self.monitors[mon_idx];
        mon.desktops.remove(desk_idx);
        mon.current = remap_index_after_removal(mon.current, desk_idx);
        // last_switched_desk feeds the taskbar-activation switchback; left
        // dangling it could target an out-of-range space.
        mon.last_switched_desk = remap_index_after_removal(mon.last_switched_desk, desk_idx);
        let new_current = mon.current;
        log_info!(
            "remove_space: Mon {} removed Space {} ({} windows -> Space {}), {} spaces left",
            mon_idx + 1,
            desk_idx + 1,
            occupants.len(),
            target + 1,
            len - 1
        );

        self.switch_desktop(mon_idx, new_current, None);
        true
    }

    /// Force a monitor to `count` spaces (clamped to `1..=MAX_DESKTOPS`).
    /// Used by snapshot restore; shrinking cascades windows down via
    /// `remove_space` so nothing is stranded on a deleted space.
    pub fn set_space_count(&mut self, mon_idx: usize, count: usize) {
        if mon_idx >= self.monitors.len() {
            return;
        }
        let count = count.clamp(1, MAX_DESKTOPS);
        while self.monitors[mon_idx].desktops.len() < count {
            self.monitors[mon_idx].desktops.push(Vec::new());
        }
        while self.monitors[mon_idx].desktops.len() > count {
            let last = self.monitors[mon_idx].desktops.len() - 1;
            if !self.remove_space(mon_idx, last) {
                break;
            }
        }
    }

    /// Highest space count across monitors — how many switch/move hotkeys
    /// need to be registered.
    pub fn max_space_count(&self) -> usize {
        self.monitors
            .iter()
            .map(|m| m.desktops.len())
            .max()
            .unwrap_or(1)
    }

    pub fn windows_show_all(&mut self) {
        log_info!("Restoring visibility for all managed windows");
        let show_all = self.show_all_taskbar;
        for mon in &mut self.monitors {
            for desk in &mut mon.desktops {
                for &hwnd in desk.iter() {
                    if is_valid_window(hwnd) {
                        set_window_visibility(hwnd, true, show_all);
                        set_window_state(hwnd, 0);
                    }
                }
                desk.clear();
            }
        }
        // Catch anything the tracked lists missed: system windows we cloaked
        // and windows whose validity changed since tracking.
        reclaim_orphaned_windows();
    }
}

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
struct AnimationGuard {
    saved: Option<ANIMATIONINFO>,
}

impl AnimationGuard {
    fn new() -> Self {
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
                if crate::shell_cloak::set_shell_cloak(hwnd, false) {
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
                if crate::shell_cloak::set_shell_cloak(hwnd, true) {
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

struct EnumMonitorsContext {
    monitors: Vec<MonitorState>,
    stable_ids: HashMap<String, String>,
}

unsafe extern "system" fn enum_monitors_callback(
    hmon: HMONITOR,
    _: HDC,
    _: *mut RECT,
    lparam: LPARAM,
) -> BOOL {
    let ctx = &mut *(lparam as *mut EnumMonitorsContext);
    if ctx.monitors.len() < MAX_MONITORS {
        ctx.monitors.push(MonitorState::new(hmon, &ctx.stable_ids));
    } else {
        log_warn!(
            "Monitor limit of {} reached; ignoring additional displays",
            MAX_MONITORS
        );
    }
    1
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settle_deadline_is_live_until_it_passes() {
        assert!(tick_before(1_000, 5_000));
        assert!(!tick_before(5_000, 5_000));
        assert!(!tick_before(9_000, 5_000));
    }

    #[test]
    fn settle_deadline_survives_tick_rollover() {
        // begin_settle near the 49-day rollover wraps the deadline past zero;
        // a plain `now < deadline` would report "already expired" and let the
        // scan re-home windows mid-restore.
        let now = u32::MAX - 1_000;
        let deadline = now.wrapping_add(4_000); // wraps to ~2999
        assert!(tick_before(now, deadline));
        assert!(tick_before(u32::MAX, deadline));
        assert!(!tick_before(3_000, deadline));
    }

    #[test]
    fn zero_deadline_never_settles_however_long_the_uptime() {
        // 30 days of uptime: `now` alone exceeds u32::MAX/2.
        assert!(!tick_before(30 * 24 * 60 * 60 * 1000, 0));
        assert!(!tick_before(0, 0));
    }

    #[test]
    fn removed_space_windows_go_left_and_first_space_falls_right() {
        assert_eq!(removal_migration_target(3), 2);
        assert_eq!(removal_migration_target(1), 0);
        // No left neighbour: old space 1 inherits, which becomes space 0
        // after the shift.
        assert_eq!(removal_migration_target(0), 1);
    }

    #[test]
    fn indices_remap_around_a_removed_space() {
        // Before the removed space: untouched.
        assert_eq!(remap_index_after_removal(1, 3), 1);
        // On the removed space: follows the migrated windows left.
        assert_eq!(remap_index_after_removal(3, 3), 2);
        assert_eq!(remap_index_after_removal(0, 0), 0);
        // Past the removed space: shifts down by one.
        assert_eq!(remap_index_after_removal(5, 3), 4);
        assert_eq!(remap_index_after_removal(1, 0), 0);
    }

    /// A plain visible, titled, unowned app window.
    fn app_window() -> WindowFacts {
        WindowFacts {
            style: WS_VISIBLE,
            ex_style: 0,
            class_name: "Chrome_WidgetWin_1".to_string(),
            has_title: true,
            externally_cloaked: false,
            hidden_by_us: false,
            owner: None,
        }
    }

    #[test]
    fn plain_app_window_is_eligible() {
        assert!(is_eligible(&app_window()));
    }

    #[test]
    fn tool_window_is_excluded_even_with_appwindow() {
        let mut f = app_window();
        f.ex_style = WS_EX_TOOLWINDOW;
        assert!(!is_eligible(&f));
        f.ex_style = WS_EX_TOOLWINDOW | WS_EX_APPWINDOW;
        assert!(!is_eligible(&f));
    }

    #[test]
    fn noactivate_is_excluded_unless_appwindow() {
        let mut f = app_window();
        f.ex_style = WS_EX_NOACTIVATE;
        assert!(!is_eligible(&f));
        f.ex_style = WS_EX_NOACTIVATE | WS_EX_APPWINDOW;
        assert!(is_eligible(&f));
    }

    #[test]
    fn shell_classes_are_excluded() {
        for class in [
            "Progman",
            "Shell_TrayWnd",
            "Windows.UI.Core.CoreWindow",
            "IME",
        ] {
            let mut f = app_window();
            f.class_name = class.to_string();
            assert!(!is_eligible(&f), "{class} must be excluded");
        }
    }

    #[test]
    fn untitled_window_is_excluded_but_untitled_owner_is_fine() {
        let mut f = app_window();
        f.has_title = false;
        assert!(!is_eligible(&f));

        let mut owned = app_window();
        let mut owner = app_window();
        owner.has_title = false;
        owned.owner = Some(Box::new(owner));
        assert!(is_eligible(&owned));
    }

    #[test]
    fn dialog_with_eligible_owner_is_included() {
        let mut f = app_window();
        f.owner = Some(Box::new(app_window()));
        assert!(is_eligible(&f));
    }

    #[test]
    fn popup_with_invisible_or_tool_window_owner_is_excluded() {
        let mut hidden_owner = app_window();
        hidden_owner.style = 0;
        let mut f = app_window();
        f.owner = Some(Box::new(hidden_owner));
        assert!(!is_eligible(&f));

        let mut tool_owner = app_window();
        tool_owner.ex_style = WS_EX_TOOLWINDOW;
        f.owner = Some(Box::new(tool_owner));
        assert!(!is_eligible(&f));
    }

    #[test]
    fn dialog_of_owner_we_cloaked_stays_eligible() {
        // Space switched away while a dialog was up: the owner is cloaked by
        // us but the dialog must remain manageable so it cloaks too.
        let mut cloaked_owner = app_window();
        cloaked_owner.hidden_by_us = true;
        cloaked_owner.style = 0;
        let mut f = app_window();
        f.owner = Some(Box::new(cloaked_owner));
        assert!(is_eligible(&f));
    }

    #[test]
    fn own_cloak_state_bit_keeps_window_eligible() {
        let mut f = app_window();
        f.hidden_by_us = true;
        f.style = 0;
        assert!(is_eligible(&f));
    }

    #[test]
    fn externally_cloaked_window_is_excluded() {
        let mut f = app_window();
        f.externally_cloaked = true;
        assert!(!is_eligible(&f));
    }

    #[test]
    fn invisible_window_without_our_state_is_excluded() {
        let mut f = app_window();
        f.style = 0;
        assert!(!is_eligible(&f));
    }
}
