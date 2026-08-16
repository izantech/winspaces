//! Tiling engine operations and SpaceManager integration.

use windows_sys::Win32::Foundation::HWND;
use windows_sys::Win32::Graphics::Dwm::{DwmGetWindowAttribute, DWMWA_EXTENDED_FRAME_BOUNDS};
use windows_sys::Win32::UI::WindowsAndMessaging::{GetForegroundWindow, IsIconic, IsZoomed};
use winspaces_common::{log_info, log_warn, WindowRect};

use super::algorithms::compute;
use super::apply::apply_layout;
use super::membership::reconcile_order;
use super::notify::schedule_retile;
use crate::spaces::{is_live_window, is_tileable_window, SpaceManager};

impl SpaceManager {
    /// Whether `hwnd` is currently placed and managed by dynamic tiling.
    ///
    /// When true, layout-restore enforcement (`try_enforce_restore`, `enforce_restore_pass`,
    /// `heal_restored_placement`) must early-return to prevent fighting the tiler.
    pub fn tiling_owns_window(&self, hwnd: HWND) -> bool {
        if !self.tiling_enabled {
            return false;
        }
        if self.is_sticky(hwnd) {
            return false;
        }
        if let Some((m_idx, s_idx)) = self.find_window(hwnd) {
            if let Some(mon) = self.monitors.get(m_idx) {
                if let Some(ts) = mon.tiling.get(s_idx) {
                    if !ts.floating.contains(&hwnd) && is_tileable_window(hwnd) {
                        return true;
                    }
                }
            }
        }
        false
    }

    /// Mark a specific space's tiling state as dirty and schedule retiling if enabled.
    pub fn mark_tiling_dirty(&mut self, mon_idx: usize, space_idx: usize) {
        if let Some(mon) = self.monitors.get_mut(mon_idx) {
            if let Some(ts) = mon.tiling.get_mut(space_idx) {
                ts.dirty = true;
                if self.tiling_enabled {
                    schedule_retile();
                }
            }
        }
    }

    /// Mark all spaces across all monitors as dirty and schedule retiling if enabled.
    pub fn mark_all_tiling_dirty(&mut self) {
        for mon in &mut self.monitors {
            for ts in &mut mon.tiling {
                ts.dirty = true;
            }
        }
        if self.tiling_enabled {
            schedule_retile();
        }
    }

    /// Toggle global dynamic tiling on or off.
    pub fn set_tiling_enabled(&mut self, enabled: bool) {
        if self.tiling_enabled == enabled {
            return;
        }
        self.tiling_enabled = enabled;
        log_info!("Tiling global enable state changed: {}", enabled);

        if enabled {
            self.mark_all_tiling_dirty();
        } else {
            // Disabled: restore default corner rounding and clear tiler target state
            for mon in &mut self.monitors {
                for ts in &mut mon.tiling {
                    for &hwnd in ts.expected.keys() {
                        if is_live_window(hwnd) {
                            unsafe {
                                winspaces_win32::dwm::set_corner_rounding(hwnd, true);
                            }
                        }
                    }
                    ts.expected.clear();
                    ts.strikes.clear();
                    ts.dirty = false;
                }
            }
        }
    }

    /// Retile all visible dirty spaces across attached monitors.
    pub fn flush_retile(&mut self) {
        if !self.tiling_enabled {
            return;
        }

        let fg = unsafe { GetForegroundWindow() };

        for m_idx in 0..self.monitors.len() {
            let cur_space = self.monitors[m_idx].current;
            if !self.monitors[m_idx].tiling[cur_space].dirty {
                continue;
            }

            let work_rect = WindowRect {
                left: self.monitors[m_idx].work.left,
                top: self.monitors[m_idx].work.top,
                right: self.monitors[m_idx].work.right,
                bottom: self.monitors[m_idx].work.bottom,
            };

            // Filter candidates: managed windows on current space, tile-eligible, not floating, not sticky, not minimized or maximized
            let candidates: Vec<HWND> = self.monitors[m_idx].spaces[cur_space]
                .iter()
                .copied()
                .filter(|&h| {
                    is_live_window(h)
                        && is_tileable_window(h)
                        && !self.monitors[m_idx].tiling[cur_space].floating.contains(&h)
                        && !self.is_sticky(h)
                        && unsafe { IsIconic(h) == 0 && IsZoomed(h) == 0 }
                })
                .collect();

            let ts = &mut self.monitors[m_idx].tiling[cur_space];
            let fg_opt = if !fg.is_null() && is_live_window(fg) {
                Some(fg)
            } else {
                None
            };
            ts.order = reconcile_order(&ts.order, &candidates, fg_opt);

            let rects = compute(
                ts.layout,
                &work_rect,
                ts.order.len(),
                &ts.ratios,
                &self.tiling_gaps,
            );

            let placements: Vec<(HWND, WindowRect)> = ts.order.iter().copied().zip(rects).collect();

            apply_layout(&placements);

            ts.expected = placements.into_iter().collect();
            ts.dirty = false;

            log_info!(
                "flush_retile: Mon {} Space {} retiled {} window(s)",
                m_idx + 1,
                cur_space + 1,
                ts.order.len()
            );
        }

        self.begin_settle(500);
    }

    /// Verification sweep checking if tiled windows accepted their assigned frames.
    /// Windows that resist twice (e.g. min-size constraints or elevated processes) are auto-floated.
    pub fn verify_retile(&mut self) {
        if !self.tiling_enabled {
            return;
        }

        let mut need_retile = false;

        for m_idx in 0..self.monitors.len() {
            let cur_space = self.monitors[m_idx].current;
            let ts = &mut self.monitors[m_idx].tiling[cur_space];

            let mut auto_floated = Vec::new();

            for (&hwnd, expected) in &ts.expected {
                if !is_live_window(hwnd) {
                    continue;
                }

                if let Some(actual) = actual_frame_bounds(hwnd) {
                    let dx = (actual.left - expected.left).abs();
                    let dy = (actual.top - expected.top).abs();
                    let dw = (actual.width() - expected.width()).abs();
                    let dh = (actual.height() - expected.height()).abs();

                    // Tolerance of 2px for DWM frame calculations
                    if dx > 2 || dy > 2 || dw > 2 || dh > 2 {
                        let strikes = ts.strikes.entry(hwnd).or_insert(0);
                        *strikes += 1;
                        if *strikes == 1 {
                            log_info!(
                                "verify_retile: hwnd {:?} mismatch (actual={:?}, expected={:?}), strike 1 -> scheduling retry",
                                hwnd,
                                actual,
                                expected
                            );
                            ts.dirty = true;
                            need_retile = true;
                        } else if *strikes >= 2 {
                            auto_floated.push(hwnd);
                        }
                    } else {
                        ts.strikes.remove(&hwnd);
                    }
                }
            }

            for hwnd in auto_floated {
                log_warn!(
                    "verify_retile: hwnd {:?} resisted tiling twice; auto-floating and restoring corner rounding",
                    hwnd
                );
                ts.floating.insert(hwnd);
                ts.expected.remove(&hwnd);
                ts.strikes.remove(&hwnd);
                unsafe {
                    winspaces_win32::dwm::set_corner_rounding(hwnd, true);
                }
                ts.dirty = true;
                need_retile = true;
            }
        }

        if need_retile {
            schedule_retile();
        }
    }
}

fn actual_frame_bounds(hwnd: HWND) -> Option<WindowRect> {
    unsafe {
        let mut frame_rect = windows_sys::Win32::Foundation::RECT {
            left: 0,
            top: 0,
            right: 0,
            bottom: 0,
        };
        if DwmGetWindowAttribute(
            hwnd,
            DWMWA_EXTENDED_FRAME_BOUNDS as _,
            &mut frame_rect as *mut _ as _,
            std::mem::size_of::<windows_sys::Win32::Foundation::RECT>() as u32,
        ) == 0
        {
            Some(WindowRect {
                left: frame_rect.left,
                top: frame_rect.top,
                right: frame_rect.right,
                bottom: frame_rect.bottom,
            })
        } else {
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::spaces::MonitorState;
    use crate::tiling::{Gaps, TileSpace};
    use std::collections::HashSet;
    use windows_sys::Win32::Foundation::RECT;

    fn test_manager_tiling(spaces: Vec<Vec<HWND>>) -> SpaceManager {
        let spaces_count = spaces.len();
        SpaceManager {
            monitors: vec![MonitorState {
                hmon: 1 as _,
                device: "\\\\.\\DISPLAY1".into(),
                stable_id: "mon-1".into(),
                rect: RECT {
                    left: 0,
                    top: 0,
                    right: 1920,
                    bottom: 1080,
                },
                work: RECT {
                    left: 0,
                    top: 0,
                    right: 1920,
                    bottom: 1040,
                },
                current: 0,
                last_switched_space: 0,
                last_switch_time: 0,
                suppress_foreground_until: 0,
                spaces,
                tiling: vec![TileSpace::new(); spaces_count],
            }],
            handle_hotkeys: true,
            show_all_taskbar: true,
            sticky_windows: HashSet::new(),
            space_indicator: true,
            suppress_foreground: false,
            reconcile_pending: false,
            suppress_rehome_until: 0,
            last_scan_tick: 0,
            restore_targets: Vec::new(),
            enforce_restore_until: 0,
            enforce_restore_ms: 0,
            enforce_restore_cap: 0,
            tiling_enabled: false,
            tiling_gaps: Gaps::NONE,
        }
    }

    #[test]
    fn tiling_enabled_toggle_marks_dirty_and_clears_expected() {
        let mut mgr = test_manager_tiling(vec![vec![100 as HWND], vec![200 as HWND]]);
        assert!(!mgr.tiling_enabled);
        assert!(!mgr.monitors[0].tiling[0].dirty);

        mgr.set_tiling_enabled(true);
        assert!(mgr.tiling_enabled);
        assert!(mgr.monitors[0].tiling[0].dirty);
        assert!(mgr.monitors[0].tiling[1].dirty);

        mgr.monitors[0].tiling[0].expected.insert(
            100 as HWND,
            WindowRect {
                left: 0,
                top: 0,
                right: 960,
                bottom: 1040,
            },
        );
        mgr.monitors[0].tiling[0].strikes.insert(100 as HWND, 1);

        mgr.set_tiling_enabled(false);
        assert!(!mgr.tiling_enabled);
        assert!(mgr.monitors[0].tiling[0].expected.is_empty());
        assert!(mgr.monitors[0].tiling[0].strikes.is_empty());
        assert!(!mgr.monitors[0].tiling[0].dirty);
    }

    #[test]
    fn space_operations_maintain_parallel_tiling_invariant() {
        let mut mgr = test_manager_tiling(vec![vec![100 as HWND], vec![200 as HWND]]);
        assert_eq!(mgr.monitors[0].spaces.len(), mgr.monitors[0].tiling.len());

        // Add space
        mgr.add_space(0);
        assert_eq!(mgr.monitors[0].spaces.len(), 3);
        assert_eq!(mgr.monitors[0].tiling.len(), 3);

        // Reorder space
        mgr.monitors[0].tiling[0].ratios.push(0.75);
        mgr.reorder_space(0, 0, 2);
        assert_eq!(mgr.monitors[0].spaces.len(), 3);
        assert_eq!(mgr.monitors[0].tiling.len(), 3);
        assert_eq!(mgr.monitors[0].tiling[2].ratios.get(0), Some(&0.75));

        // Set space count
        mgr.set_space_count(0, 5);
        assert_eq!(mgr.monitors[0].spaces.len(), 5);
        assert_eq!(mgr.monitors[0].tiling.len(), 5);
    }
}
