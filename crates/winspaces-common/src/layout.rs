//! Per-topology layout snapshots.
//!
//! A display topology (the set of physically attached monitors) can be torn
//! down and rebuilt underneath a running session — most aggressively by RDP,
//! which detaches every physical display and substitutes a single virtual one.
//! Windows reflows every top-level window onto whatever survives, and nothing
//! puts them back.
//!
//! A [`TopologySnapshot`] records where every managed window lived under one
//! specific topology: which physical monitor (by stable device path, not by
//! enumeration order), which space, and what geometry. [`LayoutStore`] keeps one
//! snapshot per topology so returning to a known arrangement can replay it.
//!
//! Geometry is stored twice on purpose: `rect` is the exact physical-pixel rect
//! for a pixel-perfect replay onto an unchanged monitor, and `rel` is the same
//! rect as fractions of the work area so a monitor that came back at a different
//! resolution or scale factor still gets a sane placement.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

use crate::{write_json_atomic, WindowRect, WorkspaceRule};

/// Most topologies retained before the least recently captured is dropped.
const MAX_TOPOLOGIES: usize = 8;

/// A rect expressed as fractions of a monitor work area.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, Default, PartialEq)]
pub struct RelRect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl RelRect {
    pub fn from_abs(rect: &WindowRect, work: &WindowRect) -> Self {
        let ww = work.width().max(1) as f32;
        let wh = work.height().max(1) as f32;
        Self {
            x: (rect.left - work.left) as f32 / ww,
            y: (rect.top - work.top) as f32 / wh,
            w: rect.width() as f32 / ww,
            h: rect.height() as f32 / wh,
        }
    }

    pub fn to_abs(&self, work: &WindowRect) -> WindowRect {
        let ww = work.width().max(1) as f32;
        let wh = work.height().max(1) as f32;
        let left = work.left + (self.x * ww).round() as i32;
        let top = work.top + (self.y * wh).round() as i32;
        WindowRect {
            left,
            top,
            right: left + (self.w * ww).round() as i32,
            bottom: top + (self.h * wh).round() as i32,
        }
    }
}

/// Keep `rect` inside `work`, shrinking it first if it simply does not fit.
/// Guarantees the window stays reachable with a mouse rather than being
/// restored to coordinates that no longer exist.
pub fn clamp_to_work(rect: &WindowRect, work: &WindowRect) -> WindowRect {
    let w = rect.width().clamp(1, work.width().max(1));
    let h = rect.height().clamp(1, work.height().max(1));
    let left = rect.left.clamp(work.left, (work.right - w).max(work.left));
    let top = rect.top.clamp(work.top, (work.bottom - h).max(work.top));
    WindowRect {
        left,
        top,
        right: left + w,
        bottom: top + h,
    }
}

/// One monitor as it existed when the snapshot was taken.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct MonitorSnapshot {
    /// Stable device path from `QueryDisplayConfig`, e.g.
    /// `\\?\DISPLAY#BNQ805B#5&1f33c64f&0&UID4354#{...}`. Survives the
    /// `\\.\DISPLAYn` slot renumbering that RDP and docking cause.
    pub stable_id: String,
    /// GDI device name at capture time. Diagnostic only — never a key.
    #[serde(default)]
    pub device: String,
    pub rect: WindowRect,
    pub work: WindowRect,
    pub dpi: u32,
    pub current_space: usize,
    /// How many spaces this monitor had when the snapshot was taken. Snapshots
    /// written before counts became dynamic deserialize to the old fixed 4.
    #[serde(default = "default_space_count")]
    pub space_count: usize,
}

fn default_space_count() -> usize {
    crate::DEFAULT_DESKTOPS
}

/// One window's home under a given topology. The first four fields are the
/// same fingerprint [`WorkspaceRule`] uses, so the existing rule-scoring path
/// matches snapshots without a second implementation.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct WindowSnapshot {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub aumid: String,
    pub exe_path: String,
    pub class_name: String,
    #[serde(default)]
    pub title_pattern: String,
    pub stable_monitor_id: String,
    pub space_index: usize,
    pub show_cmd: u32,
    #[serde(default)]
    pub is_snapped: bool,
    pub rect: WindowRect,
    #[serde(default)]
    pub rel: RelRect,
    #[serde(default)]
    pub dpi: u32,
}

impl WindowSnapshot {
    /// Project this snapshot onto a live monitor. When the monitor came back
    /// with the same work area and scale factor — the common case, and the only
    /// way to get a pixel-exact restore — the captured rect is replayed
    /// verbatim. Otherwise the work-area fractions are reprojected, so a
    /// monitor that returned at a different resolution or DPI still lands the
    /// window in proportionally the right place.
    pub fn resolve_rect(&self, captured: &MonitorSnapshot, live: &MonitorSnapshot) -> WindowRect {
        let rect = if captured.work == live.work && captured.dpi == live.dpi {
            self.rect.clone()
        } else {
            self.rel.to_abs(&live.work)
        };
        clamp_to_work(&rect, &live.work)
    }

    /// Compare everything that decides *where a window belongs*, ignoring
    /// `name`. `name` embeds the live window title, which changes every time a
    /// browser tab or chat thread changes — letting it mark the shadow dirty
    /// would rewrite the layout file continuously while nothing moved.
    pub fn same_placement(&self, other: &Self) -> bool {
        self.aumid == other.aumid
            && self.exe_path == other.exe_path
            && self.class_name == other.class_name
            && self.title_pattern == other.title_pattern
            && self.stable_monitor_id == other.stable_monitor_id
            && self.space_index == other.space_index
            && self.show_cmd == other.show_cmd
            && self.is_snapped == other.is_snapped
            && self.rect == other.rect
            && self.dpi == other.dpi
    }

    /// Adapt to a [`WorkspaceRule`] so `workspaces::score_rule` and
    /// `apply_rule_to_window` can be reused unchanged.
    pub fn as_rule(&self, rect: WindowRect) -> WorkspaceRule {
        WorkspaceRule {
            name: self.name.clone(),
            aumid: self.aumid.clone(),
            exe_path: self.exe_path.clone(),
            class_name: self.class_name.clone(),
            title_pattern: self.title_pattern.clone(),
            display_index: 0,
            desktop_index: self.space_index,
            show_cmd: self.show_cmd,
            rect,
            is_snapped: self.is_snapped,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TopologySnapshot {
    pub signature: String,
    pub monitors: Vec<MonitorSnapshot>,
    pub windows: Vec<WindowSnapshot>,
    #[serde(default)]
    pub captured_unix: u64,
}

impl TopologySnapshot {
    pub fn monitor(&self, stable_id: &str) -> Option<&MonitorSnapshot> {
        self.monitors.iter().find(|m| m.stable_id == stable_id)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct LayoutStore {
    #[serde(default)]
    pub topologies: Vec<TopologySnapshot>,
}

impl LayoutStore {
    /// `layouts.json`, sibling of `settings.json`. Deliberately a separate file:
    /// layout shadowing writes far more often than settings do, and a torn write
    /// must never cost the user their hotkeys or capture rules.
    pub fn get_path() -> PathBuf {
        crate::config_dir().join("layouts.json")
    }

    pub fn load_from_file(path: &Path) -> Self {
        match std::fs::read_to_string(path) {
            Ok(content) => serde_json::from_str(&content).unwrap_or_default(),
            Err(_) => Self::default(),
        }
    }

    pub fn save_to_file(&self, path: &Path) -> std::io::Result<()> {
        write_json_atomic(path, self)
    }

    pub fn find(&self, signature: &str) -> Option<&TopologySnapshot> {
        self.topologies.iter().find(|t| t.signature == signature)
    }

    /// Replace the snapshot for this signature, evicting the least recently
    /// captured topology once the cap is reached.
    pub fn upsert(&mut self, snapshot: TopologySnapshot) {
        if let Some(slot) = self
            .topologies
            .iter_mut()
            .find(|t| t.signature == snapshot.signature)
        {
            *slot = snapshot;
            return;
        }
        if self.topologies.len() >= MAX_TOPOLOGIES {
            if let Some((idx, _)) = self
                .topologies
                .iter()
                .enumerate()
                .min_by_key(|(_, t)| t.captured_unix)
            {
                self.topologies.remove(idx);
            }
        }
        self.topologies.push(snapshot);
    }
}

pub fn unix_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn work(left: i32, top: i32, right: i32, bottom: i32) -> WindowRect {
        WindowRect {
            left,
            top,
            right,
            bottom,
        }
    }

    fn rects_close(a: &WindowRect, b: &WindowRect, tol: i32) -> bool {
        (a.left - b.left).abs() <= tol
            && (a.top - b.top).abs() <= tol
            && (a.right - b.right).abs() <= tol
            && (a.bottom - b.bottom).abs() <= tol
    }

    #[test]
    fn rel_rect_round_trips_on_the_same_work_area() {
        let w = work(0, 0, 3840, 2508);
        let r = work(366, 537, 2882, 1950);
        let rel = RelRect::from_abs(&r, &w);
        assert_eq!(rel.to_abs(&w), r);
    }

    #[test]
    fn rel_rect_projects_across_resolutions() {
        // Left half of a 3840x2560 desk monitor replayed onto a 1920x1080
        // RDP display: still the left half, still full height.
        let big = work(0, 0, 3840, 2508);
        let small = work(0, 0, 1920, 1032);
        let half = work(0, 0, 1920, 2508);
        let rel = RelRect::from_abs(&half, &big);
        let projected = rel.to_abs(&small);
        assert_eq!(projected, work(0, 0, 960, 1032));
    }

    #[test]
    fn rel_rect_handles_negative_origin_monitors() {
        // The secondary sits at x=-1920 in physical coordinates.
        let w = work(-1920, 667, 0, 1815);
        let r = work(-968, 667, 8, 1815);
        let rel = RelRect::from_abs(&r, &w);
        let back = rel.to_abs(&w);
        assert!(rects_close(&back, &r, 1), "{back:?} != {r:?}");
    }

    #[test]
    fn clamp_pulls_offscreen_rects_back_into_the_work_area() {
        let w = work(0, 0, 2560, 1400);
        // Captured on a monitor that is no longer there.
        let stale = work(-1920, 667, -944, 1815);
        let clamped = clamp_to_work(&stale, &w);
        assert!(clamped.left >= w.left && clamped.top >= w.top);
        assert!(clamped.right <= w.right && clamped.bottom <= w.bottom);
        // Size preserved where it fits.
        assert_eq!(clamped.width(), 976);
    }

    #[test]
    fn clamp_shrinks_windows_larger_than_the_work_area() {
        let w = work(0, 0, 1920, 1032);
        let huge = work(366, 537, 2882, 1950);
        let clamped = clamp_to_work(&huge, &w);
        assert_eq!(clamped, work(0, 0, 1920, 1032));
    }

    #[test]
    fn resolve_rect_is_pixel_exact_when_nothing_moved() {
        let live = MonitorSnapshot {
            stable_id: "mon-a".into(),
            device: "\\\\.\\DISPLAY2".into(),
            rect: work(0, 0, 3840, 2560),
            work: work(0, 0, 3840, 2508),
            dpi: 144,
            current_space: 0,
            space_count: 4,
        };
        let r = work(366, 537, 2882, 1950);
        let snap = WindowSnapshot {
            name: "Fork".into(),
            aumid: String::new(),
            exe_path: "C:\\Fork.exe".into(),
            class_name: "HwndWrapper".into(),
            title_pattern: String::new(),
            stable_monitor_id: "mon-a".into(),
            space_index: 2,
            show_cmd: 1,
            is_snapped: false,
            rect: r.clone(),
            rel: RelRect::from_abs(&r, &live.work),
            dpi: 144,
        };
        assert_eq!(snap.resolve_rect(&live, &live), r);
    }

    #[test]
    fn resolve_rect_reprojects_when_the_monitor_came_back_smaller() {
        let captured = MonitorSnapshot {
            stable_id: "mon-a".into(),
            device: String::new(),
            rect: work(0, 0, 3840, 2560),
            work: work(0, 0, 3840, 2508),
            dpi: 144,
            current_space: 0,
            space_count: 4,
        };
        let r = work(0, 0, 1920, 2508);
        let snap = WindowSnapshot {
            name: "Half".into(),
            aumid: String::new(),
            exe_path: "C:\\a.exe".into(),
            class_name: "C".into(),
            title_pattern: String::new(),
            stable_monitor_id: "mon-a".into(),
            space_index: 0,
            show_cmd: 1,
            is_snapped: true,
            rel: RelRect::from_abs(&r, &captured.work),
            rect: r,
            dpi: 144,
        };
        let live = MonitorSnapshot {
            stable_id: "mon-a".into(),
            device: String::new(),
            rect: work(0, 0, 1920, 1080),
            work: work(0, 0, 1920, 1032),
            dpi: 96,
            current_space: 0,
            space_count: 4,
        };
        assert_eq!(snap.resolve_rect(&captured, &live), work(0, 0, 960, 1032));
    }

    #[test]
    fn monitor_snapshot_without_space_count_defaults_to_four() {
        // layouts.json written before dynamic desktops has no space_count.
        let json = r#"{
            "stable_id": "mon-a",
            "rect": {"left": 0, "top": 0, "right": 1920, "bottom": 1080},
            "work": {"left": 0, "top": 0, "right": 1920, "bottom": 1032},
            "dpi": 96,
            "current_space": 2
        }"#;
        let snap: MonitorSnapshot = serde_json::from_str(json).unwrap();
        assert_eq!(snap.space_count, crate::DEFAULT_DESKTOPS);
    }

    #[test]
    fn upsert_replaces_matching_signature() {
        let mut store = LayoutStore::default();
        let mk = |sig: &str, ts: u64| TopologySnapshot {
            signature: sig.into(),
            monitors: Vec::new(),
            windows: Vec::new(),
            captured_unix: ts,
        };
        store.upsert(mk("a", 1));
        store.upsert(mk("a", 2));
        assert_eq!(store.topologies.len(), 1);
        assert_eq!(store.topologies[0].captured_unix, 2);
    }

    #[test]
    fn upsert_evicts_the_least_recently_captured() {
        let mut store = LayoutStore::default();
        for i in 0..MAX_TOPOLOGIES {
            store.upsert(TopologySnapshot {
                signature: format!("sig-{i}"),
                monitors: Vec::new(),
                windows: Vec::new(),
                captured_unix: (i as u64) + 10,
            });
        }
        store.upsert(TopologySnapshot {
            signature: "fresh".into(),
            monitors: Vec::new(),
            windows: Vec::new(),
            captured_unix: 999,
        });
        assert_eq!(store.topologies.len(), MAX_TOPOLOGIES);
        assert!(store.find("sig-0").is_none());
        assert!(store.find("fresh").is_some());
    }

    #[test]
    fn store_load_tolerates_a_corrupt_file() {
        // Never destroy or panic on hand-edited/truncated layout data; a bad
        // store just means "no known topologies" until the next capture.
        let dir = std::env::temp_dir().join("winspaces-layout-test");
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("corrupt.json");
        std::fs::write(&path, "{ not json").unwrap();
        assert_eq!(LayoutStore::load_from_file(&path), LayoutStore::default());
        let _ = std::fs::remove_file(&path);
    }
}
