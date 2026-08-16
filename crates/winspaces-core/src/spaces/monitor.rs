//! Per-monitor space state and monitor enumeration.

use std::collections::HashMap;
use windows_sys::Win32::Foundation::{BOOL, HWND, LPARAM, POINT, RECT};
use windows_sys::Win32::Graphics::Gdi::{GetMonitorInfoW, HDC, HMONITOR, MONITORINFOEXW};
use windows_sys::Win32::UI::HiDpi::{GetDpiForMonitor, MDT_EFFECTIVE_DPI};
use winspaces_common::{log_warn, DEFAULT_SPACES};

/// Hard cap on tracked monitors: `EnumDisplayMonitors` growth beyond this is
/// ignored rather than growing the per-monitor tables unboundedly.
pub const MAX_MONITORS: usize = 8;

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
    pub last_switched_space: usize,
    pub last_switch_time: u32,
    pub suppress_foreground_until: u32,
    /// One entry per space; `spaces.len()` IS this monitor's space count
    /// (always in `1..=MAX_SPACES`). Counts are per monitor, so every bounds
    /// check must go through this length, never a global constant.
    pub spaces: Vec<Vec<HWND>>,
    /// Parallel to `spaces`, same length INVARIANT maintained at all 6 touch points.
    pub tiling: Vec<crate::tiling::TileSpace>,
}

/// `(szDevice, rcMonitor, rcWork)` for a monitor handle.
fn monitor_geometry(hmon: HMONITOR) -> (String, RECT, RECT) {
    unsafe {
        let mut mi: MONITORINFOEXW = std::mem::zeroed();
        mi.monitorInfo.cbSize = std::mem::size_of::<MONITORINFOEXW>() as u32;
        if GetMonitorInfoW(hmon, &mut mi as *mut _ as *mut _) != 0 {
            (
                winspaces_win32::text::wide_to_string(&mi.szDevice),
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
            last_switched_space: 0,
            last_switch_time: 0,
            suppress_foreground_until: 0,
            spaces: vec![Vec::new(); DEFAULT_SPACES],
            tiling: vec![crate::tiling::TileSpace::new(); DEFAULT_SPACES],
        }
    }

    pub(crate) fn contains(&self, pt: POINT) -> bool {
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

pub(crate) struct EnumMonitorsContext {
    pub(crate) monitors: Vec<MonitorState>,
    pub(crate) stable_ids: HashMap<String, String>,
}

pub(crate) unsafe extern "system" fn enum_monitors_callback(
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
