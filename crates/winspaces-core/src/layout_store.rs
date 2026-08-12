//! Capturing and replaying window layouts per display topology.
//!
//! The daemon keeps a *shadow* of the current layout, refreshed while the
//! topology is healthy, and persists one snapshot per topology signature. When
//! a known topology comes back — the user sits down and the two desk monitors
//! reattach — the snapshot is replayed so every window returns to its monitor,
//! its space and its geometry.
//!
//! Capture deliberately reuses `workspaces::capture_active_workspace`, so the
//! fingerprinting, naming and snap detection have exactly one implementation.

use std::collections::HashMap;

use windows_sys::Win32::Foundation::{HWND, RECT};
use windows_sys::Win32::UI::WindowsAndMessaging::EnumWindows;
use winspaces_common::{
    unix_now, MonitorSnapshot, RelRect, TopologySnapshot, WindowRect, WindowSnapshot,
};

use crate::desktop::{is_valid_window, DesktopManager};
use crate::workspaces;
use winspaces_common::{log_info, log_warn};

/// How long after a bulk placement the scan must not second-guess where a
/// window lives. Window geometry changes are asynchronous; a scan 90 ms later
/// still reads the pre-move rect.
pub const SETTLE_MS: u32 = 4000;

fn to_window_rect(r: &RECT) -> WindowRect {
    WindowRect {
        left: r.left,
        top: r.top,
        right: r.right,
        bottom: r.bottom,
    }
}

/// Describe the live monitors exactly as they would be recorded in a snapshot,
/// so captured and live geometry are directly comparable.
pub fn live_monitors(mgr: &DesktopManager) -> Vec<MonitorSnapshot> {
    mgr.monitors
        .iter()
        .map(|m| MonitorSnapshot {
            stable_id: m.stable_id.clone(),
            device: m.device.clone(),
            rect: to_window_rect(&m.rect),
            work: to_window_rect(&m.work),
            dpi: m.dpi(),
            current_space: m.current,
            space_count: m.desktops.len(),
        })
        .collect()
}

/// Apply each monitor's persisted space count from `snapshot`, matched by
/// stable id. Separate from `restore_snapshot` because counts are structural,
/// not layout: they must come back even when auto-restore is off.
pub fn apply_space_counts(mgr: &mut DesktopManager, snapshot: &TopologySnapshot) {
    for idx in 0..mgr.monitors.len() {
        if let Some(snap_mon) = snapshot.monitor(&mgr.monitors[idx].stable_id) {
            mgr.set_space_count(idx, snap_mon.space_count);
        }
    }
}

/// Snapshot the current layout under the current topology signature.
pub fn capture_snapshot(mgr: &DesktopManager) -> TopologySnapshot {
    let monitors = live_monitors(mgr);
    let rules = unsafe { workspaces::capture_active_workspace(mgr) };

    let mut windows: Vec<WindowSnapshot> = rules
        .into_iter()
        .filter_map(|r| {
            let mon = monitors.get(r.display_index)?;
            Some(WindowSnapshot {
                name: r.name,
                aumid: r.aumid,
                exe_path: r.exe_path,
                class_name: r.class_name,
                title_pattern: r.title_pattern,
                stable_monitor_id: mon.stable_id.clone(),
                space_index: r.desktop_index,
                show_cmd: r.show_cmd,
                is_snapped: r.is_snapped,
                rel: RelRect::from_abs(&r.rect, &mon.work),
                rect: r.rect,
                dpi: mon.dpi,
            })
        })
        .collect();

    // `capture_active_workspace` walks EnumWindows, which returns z-order — so
    // merely focusing a different window reorders the list. Without a canonical
    // order every tick compares unequal and rewrites the file forever.
    windows.sort_by(|a, b| {
        a.stable_monitor_id
            .cmp(&b.stable_monitor_id)
            .then(a.space_index.cmp(&b.space_index))
            .then(a.exe_path.cmp(&b.exe_path))
            .then(a.class_name.cmp(&b.class_name))
            .then(a.rect.left.cmp(&b.rect.left))
            .then(a.rect.top.cmp(&b.rect.top))
    });

    TopologySnapshot {
        signature: mgr.topology_signature(),
        monitors,
        windows,
        captured_unix: unix_now(),
    }
}

/// Two snapshots describe the same arrangement. Ignores `captured_unix` and
/// each window's cosmetic `name`, so a re-capture that changed nothing does not
/// mark the shadow dirty and trigger a pointless disk write.
pub fn same_layout(a: &TopologySnapshot, b: &TopologySnapshot) -> bool {
    a.signature == b.signature
        && a.monitors == b.monitors
        && a.windows.len() == b.windows.len()
        && a.windows
            .iter()
            .zip(b.windows.iter())
            .all(|(x, y)| x.same_placement(y))
}

unsafe extern "system" fn collect_windows_proc(hwnd: HWND, lparam: isize) -> i32 {
    let out = &mut *(lparam as *mut Vec<HWND>);
    if is_valid_window(hwnd) {
        out.push(hwnd);
    }
    1
}

/// Pair each live window with at most one snapshot entry, best match first.
///
/// A plain "best rule per window" pass (what workspace-rule restore does) would
/// send every Brave window to the same snapshot entry and stack them on top of
/// each other. Ranking all candidate pairs and consuming both sides keeps N
/// windows of one app spread across the N places they came from.
fn assign_windows(
    hwnds: &[HWND],
    snapshot: &TopologySnapshot,
) -> Vec<(HWND, usize /* window index */)> {
    let mut scored: Vec<(i32, usize, usize)> = Vec::new();
    for (w_idx, hwnd) in hwnds.iter().enumerate() {
        let (aumid, exe_path, class_name, title) = unsafe {
            (
                workspaces::get_window_aumid(*hwnd).to_lowercase(),
                workspaces::get_process_image_path(*hwnd).to_lowercase(),
                workspaces::get_window_class(*hwnd).to_lowercase(),
                workspaces::get_window_title(*hwnd).to_lowercase(),
            )
        };
        for (s_idx, snap) in snapshot.windows.iter().enumerate() {
            let rule = snap.as_rule(snap.rect.clone());
            if let Some(score) =
                workspaces::score_rule(&aumid, &exe_path, &class_name, &title, &rule)
            {
                scored.push((score, w_idx, s_idx));
            }
        }
    }

    resolve_pairs(scored, hwnds.len(), snapshot.windows.len())
        .into_iter()
        .map(|(w_idx, s_idx)| (hwnds[w_idx], s_idx))
        .collect()
}

/// Greedy one-to-one assignment over `(score, window_idx, snapshot_idx)`
/// candidates: take the highest-scoring pair, retire both sides, repeat.
fn resolve_pairs(
    mut scored: Vec<(i32, usize, usize)>,
    window_count: usize,
    snapshot_count: usize,
) -> Vec<(usize, usize)> {
    // Highest score first; ties broken deterministically so a replay is
    // repeatable rather than dependent on enumeration order.
    scored.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)).then(a.2.cmp(&b.2)));

    let mut used_windows = vec![false; window_count];
    let mut used_snaps = vec![false; snapshot_count];
    let mut pairs = Vec::new();
    for (_, w_idx, s_idx) in scored {
        if used_windows[w_idx] || used_snaps[s_idx] {
            continue;
        }
        used_windows[w_idx] = true;
        used_snaps[s_idx] = true;
        pairs.push((w_idx, s_idx));
    }
    pairs
}

/// Replay `snapshot` onto the live desktop. Assumes the caller has already
/// confirmed the live topology signature matches the snapshot's.
pub fn restore_snapshot(mgr: &mut DesktopManager, snapshot: &TopologySnapshot) {
    // Counts first: placements below index into `desktops` and must see the
    // monitor at its restored size, not the default.
    apply_space_counts(mgr, snapshot);
    let live = live_monitors(mgr);
    let by_stable_id: HashMap<&str, usize> = live
        .iter()
        .enumerate()
        .map(|(idx, m)| (m.stable_id.as_str(), idx))
        .collect();

    // Geometry does not stick to a cloaked or force-minimized window, so
    // everything comes back into view before anything is moved.
    mgr.show_all_tracked();

    let mut hwnds: Vec<HWND> = Vec::new();
    unsafe {
        EnumWindows(Some(collect_windows_proc), &mut hwnds as *mut _ as isize);
    }

    let pairs = assign_windows(&hwnds, snapshot);
    let mut placed = 0usize;
    for (hwnd, s_idx) in pairs {
        let snap = &snapshot.windows[s_idx];
        let Some(&mon_idx) = by_stable_id.get(snap.stable_monitor_id.as_str()) else {
            log_warn!(
                "restore: monitor {} for '{}' is not attached; skipping",
                snap.stable_monitor_id,
                snap.name
            );
            continue;
        };
        let Some(captured) = snapshot.monitor(&snap.stable_monitor_id) else {
            continue;
        };

        let rect = snap.resolve_rect(captured, &live[mon_idx]);
        let rule = snap.as_rule(rect);
        unsafe {
            workspaces::apply_rule_to_window(hwnd, &rule, Some(mgr.monitors[mon_idx].hmon));
        }
        mgr.track_window(hwnd, mon_idx, snap.space_index);
        placed += 1;
    }

    for m in &mut mgr.monitors {
        if let Some(snap_mon) = snapshot.monitor(&m.stable_id) {
            m.current = snap_mon.current_space.min(m.desktops.len() - 1);
        }
    }
    mgr.reapply_visibility();
    mgr.begin_settle(SETTLE_MS);

    log_info!(
        "Restored layout for topology [{}]: {}/{} windows placed",
        snapshot.signature,
        placed,
        snapshot.windows.len()
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use winspaces_common::LayoutStore;

    fn rect(left: i32, top: i32, right: i32, bottom: i32) -> WindowRect {
        WindowRect {
            left,
            top,
            right,
            bottom,
        }
    }

    fn snap(name: &str, exe: &str, class: &str, title: &str) -> WindowSnapshot {
        WindowSnapshot {
            name: name.into(),
            aumid: String::new(),
            exe_path: exe.into(),
            class_name: class.into(),
            title_pattern: title.into(),
            stable_monitor_id: "mon-a".into(),
            space_index: 0,
            show_cmd: 1,
            is_snapped: false,
            rect: rect(0, 0, 100, 100),
            rel: RelRect::default(),
            dpi: 96,
        }
    }

    fn topology(windows: Vec<WindowSnapshot>) -> TopologySnapshot {
        TopologySnapshot {
            signature: "mon-a".into(),
            monitors: vec![MonitorSnapshot {
                stable_id: "mon-a".into(),
                device: "\\\\.\\DISPLAY1".into(),
                rect: rect(0, 0, 1920, 1080),
                work: rect(0, 0, 1920, 1032),
                dpi: 96,
                current_space: 0,
                space_count: 4,
            }],
            windows,
            captured_unix: 1,
        }
    }

    #[test]
    fn same_layout_ignores_the_capture_timestamp() {
        let a = topology(vec![snap("A", "a.exe", "C", "")]);
        let mut b = a.clone();
        b.captured_unix = 999;
        assert!(same_layout(&a, &b));
        b.windows[0].space_index = 2;
        assert!(!same_layout(&a, &b));
    }

    #[test]
    fn same_layout_ignores_a_changed_window_title() {
        // Switching a browser tab rewrites `name`; nothing moved, so the
        // shadow must not go dirty and rewrite layouts.json every 5 seconds.
        let a = topology(vec![snap("Gmail - Brave", "brave.exe", "C", "")]);
        let mut b = a.clone();
        b.windows[0].name = "Google Maps - Brave".into();
        assert!(same_layout(&a, &b));
    }

    #[test]
    fn same_layout_still_notices_a_real_move() {
        let a = topology(vec![snap("A", "a.exe", "C", "")]);
        let mut b = a.clone();
        b.windows[0].rect = rect(100, 100, 300, 300);
        assert!(!same_layout(&a, &b));

        let mut c = a.clone();
        c.windows[0].stable_monitor_id = "mon-b".into();
        assert!(!same_layout(&a, &c));

        let mut d = a.clone();
        d.monitors[0].current_space = 3;
        assert!(!same_layout(&a, &d));
    }

    #[test]
    fn capture_ordering_is_canonical_not_z_order() {
        // Two captures of the same desktop that differ only in the order
        // EnumWindows returned must compare equal after the canonical sort.
        let one = snap("A", "a.exe", "C1", "");
        let mut two = snap("B", "b.exe", "C2", "");
        two.space_index = 2;
        let forward = topology(vec![one.clone(), two.clone()]);
        let reversed = topology(vec![two, one]);
        assert!(!same_layout(&forward, &reversed));

        let mut sorted = reversed.windows.clone();
        sorted.sort_by(|a, b| {
            a.stable_monitor_id
                .cmp(&b.stable_monitor_id)
                .then(a.space_index.cmp(&b.space_index))
                .then(a.exe_path.cmp(&b.exe_path))
        });
        assert!(same_layout(&forward, &topology(sorted)));
    }

    #[test]
    fn assignment_is_one_to_one_for_identical_apps() {
        // Three Brave windows, three snapshot entries, every pair scoring the
        // same: each window must claim a distinct entry rather than all three
        // stacking on one rect.
        let scored: Vec<(i32, usize, usize)> = (0..3)
            .flat_map(|w| (0..3).map(move |s| (30, w, s)))
            .collect();
        assert_eq!(resolve_pairs(scored, 3, 3), vec![(0, 0), (1, 1), (2, 2)]);
    }

    #[test]
    fn assignment_prefers_the_stronger_match() {
        // Window 1 matches snapshot 0 by AUMID (130) while window 0 only
        // matches it by exe (20). The specific match must win, and window 0
        // then falls back to the entry left over.
        let scored = vec![(20, 0, 0), (130, 1, 0), (20, 0, 1), (20, 1, 1)];
        assert_eq!(resolve_pairs(scored, 2, 2), vec![(1, 0), (0, 1)]);
    }

    #[test]
    fn assignment_leaves_unmatched_windows_alone() {
        // Four live windows, two snapshot entries: only two get placed, the
        // rest keep whatever position they already had.
        let scored = vec![(30, 0, 0), (30, 1, 1)];
        assert_eq!(resolve_pairs(scored, 4, 2).len(), 2);
        assert!(resolve_pairs(Vec::new(), 4, 2).is_empty());
    }

    #[test]
    fn store_round_trips_a_snapshot_through_disk() {
        let dir = std::env::temp_dir().join("winspaces-layout-store-test");
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("layouts.json");
        let mut store = LayoutStore::default();
        store.upsert(topology(vec![snap("A", "a.exe", "C", "")]));
        store.save_to_file(&path).unwrap();

        let loaded = LayoutStore::load_from_file(&path);
        assert_eq!(loaded, store);
        assert!(loaded.find("mon-a").is_some());
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn snapshot_monitor_lookup_is_by_stable_id() {
        let t = topology(vec![]);
        assert!(t.monitor("mon-a").is_some());
        assert!(t.monitor("\\\\.\\DISPLAY1").is_none());
    }
}
