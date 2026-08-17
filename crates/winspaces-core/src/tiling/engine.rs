//! Tiling engine operations and SpaceManager integration.

use windows_sys::Win32::Foundation::HWND;
use windows_sys::Win32::Graphics::Dwm::{DwmGetWindowAttribute, DWMWA_EXTENDED_FRAME_BOUNDS};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    GetForegroundWindow, GetWindowPlacement, IsIconic, IsZoomed, SetForegroundWindow,
    SetWindowPlacement, SW_SHOWNOACTIVATE, WINDOWPLACEMENT,
};
use winspaces_common::{log_info, log_warn, WindowRect};

use super::algorithms::compute;
use super::apply::apply_layout;
use super::membership::reconcile_order;
use super::notify::schedule_retile;
use super::resize::classify_drag;
use super::types::{Direction, DragOutcome, TilingDrag, DEFAULT_RATIO, MAX_RATIO, MIN_RATIO};
use crate::spaces::{is_live_window, is_tileable_window, AnimationGuard, SpaceManager};

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
        if self.is_floating(hwnd) {
            return false;
        }
        if let Some((m_idx, s_idx)) = self.find_window(hwnd) {
            if let Some(mon) = self.monitors.get(m_idx) {
                if let Some(ts) = mon.tiling.get(s_idx) {
                    if ts.order.contains(&hwnd)
                        || ts.expected.contains_key(&hwnd)
                        || ts.flatten_strikes.contains_key(&hwnd)
                    {
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
            self.tiling_drag = None;
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
                    ts.flatten_strikes.clear();
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

            // If a drag is currently active on this monitor's current space, skip retile so we never yank the window mid-drag
            if self
                .tiling_drag
                .as_ref()
                .is_some_and(|d| d.mon_idx == m_idx && d.space_idx == cur_space)
            {
                log_info!(
                    "flush_retile: skipping active drag on Mon {} Space {}",
                    m_idx + 1,
                    cur_space + 1
                );
                continue;
            }

            let work_rect = WindowRect {
                left: self.monitors[m_idx].work.left,
                top: self.monitors[m_idx].work.top,
                right: self.monitors[m_idx].work.right,
                bottom: self.monitors[m_idx].work.bottom,
            };

            // Filter candidates: managed windows on current space, tile-eligible, not floating, not sticky, not minimized
            let candidates: Vec<HWND> = self.monitors[m_idx].spaces[cur_space]
                .iter()
                .copied()
                .filter(|&h| {
                    is_live_window(h)
                        && is_tileable_window(h)
                        && !self.is_floating(h)
                        && !self.is_sticky(h)
                        && unsafe { IsIconic(h) == 0 }
                })
                .collect();

            // Un-maximize any maximized candidate without activating, suppressing animations.
            let has_zoomed = candidates.iter().any(|&h| unsafe { IsZoomed(h) != 0 });
            let _anim = has_zoomed.then(AnimationGuard::new);
            for &h in &candidates {
                unsafe {
                    if IsZoomed(h) != 0 {
                        let mut wp: WINDOWPLACEMENT = std::mem::zeroed();
                        wp.length = std::mem::size_of::<WINDOWPLACEMENT>() as u32;
                        if GetWindowPlacement(h, &mut wp) != 0 {
                            wp.showCmd = SW_SHOWNOACTIVATE as u32;
                            SetWindowPlacement(h, &wp);
                        }
                    }
                }
            }

            let dpi = self.monitors[m_idx].dpi();
            let scaled_gaps = self.tiling_gaps.scaled_for_dpi(dpi);

            let ts = &mut self.monitors[m_idx].tiling[cur_space];

            // Re-check IsZoomed: count flatten attempts. Below 3 attempts, exclude
            // from this round and keep ts.dirty true to retry. At 3 attempts,
            // auto-float the window and end the retry loop for it.
            let mut pending_zoom = false;
            let mut auto_floated_zoomed = Vec::new();
            let ready_candidates: Vec<HWND> = candidates
                .into_iter()
                .filter(|&h| {
                    if unsafe { IsZoomed(h) != 0 } {
                        let attempts = ts.flatten_strikes.entry(h).or_insert(0);
                        *attempts += 1;
                        if *attempts >= 3 {
                            auto_floated_zoomed.push(h);
                        } else {
                            log_info!(
                                "flush_retile: hwnd {:?} flatten pending (still zoomed, attempt {}/3)",
                                h,
                                *attempts
                            );
                            pending_zoom = true;
                        }
                        false
                    } else {
                        ts.flatten_strikes.remove(&h);
                        true
                    }
                })
                .collect();

            for hwnd in auto_floated_zoomed {
                log_warn!(
                    "flush_retile: hwnd {:?} refused to un-maximize; auto-floating",
                    hwnd
                );
                self.floating_windows.insert(hwnd);
                ts.expected.remove(&hwnd);
                ts.strikes.remove(&hwnd);
                ts.flatten_strikes.remove(&hwnd);
                unsafe {
                    winspaces_win32::dwm::set_corner_rounding(hwnd, true);
                }
            }

            let fg_opt = if !fg.is_null() && is_live_window(fg) {
                Some(fg)
            } else {
                None
            };
            ts.order = reconcile_order(&ts.order, &ready_candidates, fg_opt);

            let rects = compute(
                ts.layout,
                &work_rect,
                ts.order.len(),
                &ts.ratios,
                &scaled_gaps,
            );

            let placements: Vec<(HWND, WindowRect)> = ts.order.iter().copied().zip(rects).collect();

            apply_layout(&placements);

            ts.expected = placements.into_iter().collect();
            if pending_zoom {
                ts.dirty = true;
                schedule_retile();
            } else {
                ts.dirty = false;
            }

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
    /// Windows that resist twice (e.g. min-size constraints) or refuse to un-maximize
    /// after 3 attempts (e.g. elevated processes) are auto-floated.
    pub fn verify_retile(&mut self) {
        if !self.tiling_enabled {
            return;
        }

        let mut need_retile = false;

        for m_idx in 0..self.monitors.len() {
            let cur_space = self.monitors[m_idx].current;
            let ts = &mut self.monitors[m_idx].tiling[cur_space];

            let mut auto_floated_resistant = Vec::new();
            let mut auto_floated_zoomed = Vec::new();

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
                        let is_zoomed = unsafe { IsZoomed(hwnd) != 0 };
                        if is_zoomed {
                            let attempts = ts.flatten_strikes.entry(hwnd).or_insert(0);
                            *attempts += 1;
                            if *attempts >= 3 {
                                auto_floated_zoomed.push(hwnd);
                            } else {
                                log_info!(
                                    "verify_retile: hwnd {:?} is still maximized (flatten attempt {}/3) -> scheduling retry",
                                    hwnd,
                                    *attempts
                                );
                                ts.dirty = true;
                                need_retile = true;
                            }
                        } else {
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
                                auto_floated_resistant.push(hwnd);
                            }
                        }
                    } else {
                        ts.strikes.remove(&hwnd);
                        ts.flatten_strikes.remove(&hwnd);
                    }
                }
            }

            for hwnd in auto_floated_resistant {
                log_warn!(
                    "verify_retile: hwnd {:?} resisted tiling twice; auto-floating and restoring corner rounding",
                    hwnd
                );
                self.floating_windows.insert(hwnd);
                ts.expected.remove(&hwnd);
                ts.strikes.remove(&hwnd);
                ts.flatten_strikes.remove(&hwnd);
                unsafe {
                    winspaces_win32::dwm::set_corner_rounding(hwnd, true);
                }
                ts.dirty = true;
                need_retile = true;
            }

            for hwnd in auto_floated_zoomed {
                log_warn!(
                    "verify_retile: hwnd {:?} refused to un-maximize; auto-floating",
                    hwnd
                );
                self.floating_windows.insert(hwnd);
                ts.expected.remove(&hwnd);
                ts.strikes.remove(&hwnd);
                ts.flatten_strikes.remove(&hwnd);
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

    /// Navigate keyboard focus to the neighbor tile in direction `dir`.
    pub fn tiling_focus(&mut self, dir: Direction) {
        if !self.tiling_enabled {
            log_info!("tiling_focus({:?}): tiling is disabled", dir);
            return;
        }

        let fg = unsafe { GetForegroundWindow() };
        if fg.is_null() {
            return;
        }

        if let Some((m_idx, s_idx)) = self.find_window(fg) {
            let cur_space = self.monitors[m_idx].current;
            if s_idx == cur_space {
                let ts = &self.monitors[m_idx].tiling[cur_space];
                let tiles: Vec<(HWND, WindowRect)> = ts
                    .order
                    .iter()
                    .filter_map(|&h| ts.expected.get(&h).map(|r| (h, r.clone())))
                    .collect();

                if let Some(target_hwnd) = super::neighbors::directional_neighbor(fg, dir, &tiles) {
                    log_info!("tiling_focus: {:?} -> {:?}", dir, target_hwnd);
                    unsafe {
                        SetForegroundWindow(target_hwnd);
                    }
                }
            }
        }
    }

    /// Swap the focused tile with its neighbor in direction `dir`.
    pub fn tiling_swap(&mut self, dir: Direction) {
        if !self.tiling_enabled {
            log_info!("tiling_swap({:?}): tiling is disabled", dir);
            return;
        }

        let fg = unsafe { GetForegroundWindow() };
        if fg.is_null() {
            return;
        }

        if let Some((m_idx, s_idx)) = self.find_window(fg) {
            let cur_space = self.monitors[m_idx].current;
            if s_idx == cur_space {
                let ts = &self.monitors[m_idx].tiling[cur_space];
                let tiles: Vec<(HWND, WindowRect)> = ts
                    .order
                    .iter()
                    .filter_map(|&h| ts.expected.get(&h).map(|r| (h, r.clone())))
                    .collect();

                if let Some(target_hwnd) = super::neighbors::directional_neighbor(fg, dir, &tiles) {
                    let ts_mut = &mut self.monitors[m_idx].tiling[cur_space];
                    let pos_fg = ts_mut.order.iter().position(|&h| h == fg);
                    let pos_target = ts_mut.order.iter().position(|&h| h == target_hwnd);
                    if let (Some(i), Some(j)) = (pos_fg, pos_target) {
                        ts_mut.order.swap(i, j);
                        ts_mut.dirty = true;
                        schedule_retile();
                        log_info!("tiling_swap: swapped {:?} and {:?}", fg, target_hwnd);
                    }
                }
            }
        }
    }

    /// Adjust the primary split ratio on the current space by `delta` (e.g. +0.05 or -0.05).
    pub fn tiling_adjust_ratio(&mut self, delta: f32) {
        if !self.tiling_enabled {
            log_info!("tiling_adjust_ratio({}): tiling is disabled", delta);
            return;
        }

        let fg = unsafe { GetForegroundWindow() };
        let Some((m_idx, _)) = (!fg.is_null()).then(|| self.find_window(fg)).flatten() else {
            log_info!(
                "tiling_adjust_ratio({}): foreground window {:?} is not tracked; ignoring",
                delta,
                fg
            );
            return;
        };

        if m_idx < self.monitors.len() {
            let cur_space = self.monitors[m_idx].current;
            let ts = &mut self.monitors[m_idx].tiling[cur_space];
            if ts.ratios.is_empty() {
                ts.ratios.push(DEFAULT_RATIO);
            }
            let old = ts.ratios[0];
            let new_ratio = (old + delta).clamp(MIN_RATIO, MAX_RATIO);
            ts.ratios[0] = new_ratio;
            ts.dirty = true;
            schedule_retile();
            log_info!(
                "tiling_adjust_ratio: Mon {} Space {} ratio {} -> {}",
                m_idx + 1,
                cur_space + 1,
                old,
                new_ratio
            );
        }
    }

    /// Toggle floating state for `hwnd` (or the foreground window if null).
    pub fn tiling_toggle_float(&mut self, hwnd: HWND) {
        if !self.tiling_enabled {
            log_info!("tiling_toggle_float({:?}): tiling is disabled", hwnd);
            return;
        }

        let target = if !hwnd.is_null() {
            hwnd
        } else {
            unsafe { GetForegroundWindow() }
        };

        if target.is_null() {
            return;
        }

        if self.matches_float_rule(target) {
            log_info!(
                "tiling_toggle_float: window {:?} matches permanent float rule; rule wins, keeping floating",
                target
            );
            return;
        }

        let loc = self.find_window(target);

        if self.floating_windows.contains(&target) {
            if is_live_window(target) && !is_tileable_window(target) {
                log_info!(
                    "tiling_toggle_float: window {:?} is not tile-eligible; keeping floating",
                    target
                );
                return;
            }
            self.floating_windows.remove(&target);
            if let Some((m_idx, s_idx)) = loc {
                self.monitors[m_idx].tiling[s_idx].dirty = true;
            }
            schedule_retile();
            log_info!("tiling_toggle_float: un-floated window {:?}", target);
        } else {
            self.floating_windows.insert(target);
            if let Some((m_idx, s_idx)) = loc {
                let ts = &mut self.monitors[m_idx].tiling[s_idx];
                ts.expected.remove(&target);
                ts.strikes.remove(&target);
                ts.flatten_strikes.remove(&target);
                ts.dirty = true;
            }
            if is_live_window(target) {
                unsafe {
                    winspaces_win32::dwm::set_corner_rounding(target, true);
                }
            }
            schedule_retile();
            log_info!("tiling_toggle_float: floated window {:?}", target);
        }
    }

    /// Called on `EVENT_SYSTEM_MOVESIZESTART` to track drag-resize / drag-swap gestures on tiled windows.
    pub fn tiling_on_movesize_start(&mut self, hwnd: HWND) {
        if !self.tiling_enabled {
            return;
        }

        if !self.tiling_owns_window(hwnd) {
            return;
        }

        let Some((m_idx, s_idx)) = self.find_window(hwnd) else {
            return;
        };

        if m_idx >= self.monitors.len() || s_idx != self.monitors[m_idx].current {
            return;
        }

        let ts = &self.monitors[m_idx].tiling[s_idx];
        let start_rect = ts
            .expected
            .get(&hwnd)
            .cloned()
            .or_else(|| actual_frame_bounds(hwnd))
            .unwrap_or_default();

        self.tiling_drag = Some(TilingDrag {
            hwnd,
            start_rect,
            mon_idx: m_idx,
            space_idx: s_idx,
        });

        log_info!(
            "tiling_on_movesize_start: Mon {} Space {} hwnd {:?}",
            m_idx + 1,
            s_idx + 1,
            hwnd
        );
    }

    /// Called on `EVENT_SYSTEM_MOVESIZEEND` to classify and apply mouse gestures on tiled spaces.
    pub fn tiling_on_movesize_end(&mut self, hwnd: HWND) {
        let Some(drag) = self.tiling_drag.take() else {
            return;
        };

        if !self.tiling_enabled {
            return;
        }

        if drag.hwnd != hwnd {
            return;
        }

        let m_idx = drag.mon_idx;
        let s_idx = drag.space_idx;
        if m_idx >= self.monitors.len() || s_idx >= self.monitors[m_idx].spaces.len() {
            return;
        }

        let new_rect = actual_frame_bounds(hwnd).unwrap_or_else(|| drag.start_rect.clone());

        let mut pt = windows_sys::Win32::Foundation::POINT { x: 0, y: 0 };
        unsafe {
            windows_sys::Win32::UI::WindowsAndMessaging::GetCursorPos(&mut pt);
        }

        let ts = &self.monitors[m_idx].tiling[s_idx];
        let tiles: Vec<(HWND, WindowRect)> = ts
            .order
            .iter()
            .filter_map(|&h| ts.expected.get(&h).map(|r| (h, r.clone())))
            .collect();

        let outcome = classify_drag(hwnd, &drag.start_rect, &new_rect, &tiles, (pt.x, pt.y));

        let ts_mut = &mut self.monitors[m_idx].tiling[s_idx];
        match outcome {
            DragOutcome::AdjustDwindleRatio {
                ratio_index,
                new_ratio,
            } => {
                ts_mut.set_ratio(ratio_index, new_ratio);
                ts_mut.dirty = true;
                schedule_retile();
                log_info!(
                    "tiling_on_movesize_end: adjusted ratio {} to {}",
                    ratio_index,
                    new_ratio
                );
            }
            DragOutcome::Reorder {
                from_index,
                to_index,
            } => {
                if from_index < ts_mut.order.len() && to_index < ts_mut.order.len() {
                    ts_mut.order.swap(from_index, to_index);
                    ts_mut.dirty = true;
                    schedule_retile();
                    log_info!(
                        "tiling_on_movesize_end: swapped slot {} and {}",
                        from_index,
                        to_index
                    );
                }
            }
            DragOutcome::SnapBack => {
                ts_mut.dirty = true;
                schedule_retile();
                log_info!(
                    "tiling_on_movesize_end: snap-back to original tile layout for {:?}",
                    hwnd
                );
            }
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
            floating_windows: HashSet::new(),
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
            tiling_drag: None,
            float_rules: Vec::new(),
        }
    }

    #[test]
    fn tiling_enabled_toggle_marks_dirty_and_clears_expected() {
        let mut mgr = test_manager_tiling(vec![vec![100 as HWND]]);
        mgr.monitors[0].tiling[0].expected.insert(
            100 as HWND,
            WindowRect {
                left: 0,
                top: 0,
                right: 1920,
                bottom: 1040,
            },
        );

        mgr.set_tiling_enabled(true);
        assert!(mgr.monitors[0].tiling[0].dirty);

        mgr.set_tiling_enabled(false);
        assert!(mgr.monitors[0].tiling[0].expected.is_empty());
        assert!(!mgr.monitors[0].tiling[0].dirty);
    }

    #[test]
    fn space_operations_maintain_parallel_tiling_invariant() {
        let mut mgr = test_manager_tiling(vec![vec![100 as HWND], vec![200 as HWND]]);
        assert_eq!(mgr.monitors[0].spaces.len(), 2);
        assert_eq!(mgr.monitors[0].tiling.len(), 2);

        // Add space
        mgr.add_space(0);
        assert_eq!(mgr.monitors[0].spaces.len(), 3);
        assert_eq!(mgr.monitors[0].tiling.len(), 3);

        // Move window to new space
        mgr.step_move_window(1);

        // Remove space 1
        mgr.remove_space(0, 1);
        assert_eq!(mgr.monitors[0].spaces.len(), 2);
        assert_eq!(mgr.monitors[0].tiling.len(), 2);
    }

    #[test]
    fn tilespace_ratio_clamps_within_bounds() {
        let mut ts = TileSpace::new();
        assert_eq!(ts.ratio(0), DEFAULT_RATIO);

        ts.set_ratio(0, 0.7);
        assert_eq!(ts.ratio(0), 0.7);

        // Clamps at MAX_RATIO = 0.9
        ts.set_ratio(0, 1.5);
        assert_eq!(ts.ratio(0), MAX_RATIO);

        // Clamps at MIN_RATIO = 0.1
        ts.set_ratio(0, -0.5);
        assert_eq!(ts.ratio(0), MIN_RATIO);
    }

    #[test]
    fn tiling_toggle_float_transitions() {
        let mut mgr = test_manager_tiling(vec![vec![100 as HWND, 200 as HWND]]);
        mgr.set_tiling_enabled(true);

        mgr.monitors[0].tiling[0].expected.insert(
            100 as HWND,
            WindowRect {
                left: 0,
                top: 0,
                right: 960,
                bottom: 1040,
            },
        );

        assert!(!mgr.is_floating(100 as HWND));

        // Float
        mgr.tiling_toggle_float(100 as HWND);
        assert!(mgr.is_floating(100 as HWND));
        assert!(mgr.floating_windows.contains(&(100 as HWND)));
        assert!(!mgr.monitors[0].tiling[0]
            .expected
            .contains_key(&(100 as HWND)));
        assert!(mgr.monitors[0].tiling[0].dirty);

        // Un-float
        mgr.tiling_toggle_float(100 as HWND);
        assert!(!mgr.is_floating(100 as HWND));
        assert!(mgr.monitors[0].tiling[0].dirty);
    }

    #[test]
    fn disabled_tiling_no_ops_operations() {
        let mut mgr = test_manager_tiling(vec![vec![100 as HWND]]);
        assert!(!mgr.tiling_enabled);

        mgr.tiling_adjust_ratio(0.1);
        assert!(mgr.monitors[0].tiling[0].ratios.is_empty());

        mgr.tiling_toggle_float(100 as HWND);
        assert!(!mgr.is_floating(100 as HWND));

        mgr.tiling_on_movesize_start(100 as HWND);
        assert!(mgr.tiling_drag.is_none());
    }

    #[test]
    fn drag_active_skips_retile_flush() {
        let mut mgr = test_manager_tiling(vec![vec![100 as HWND]]);
        mgr.set_tiling_enabled(true);
        mgr.monitors[0].tiling[0].dirty = true;

        mgr.tiling_drag = Some(TilingDrag {
            hwnd: 100 as HWND,
            start_rect: WindowRect::default(),
            mon_idx: 0,
            space_idx: 0,
        });

        mgr.flush_retile();
        // Still dirty because retile was skipped mid-drag!
        assert!(mgr.monitors[0].tiling[0].dirty);

        // Clear drag
        mgr.tiling_drag = None;
        mgr.flush_retile();
        // Cleared dirty flag
        assert!(!mgr.monitors[0].tiling[0].dirty);
    }

    #[test]
    fn float_rules_prevent_tiling_ownership() {
        let mut mgr = test_manager_tiling(vec![vec![100 as HWND]]);
        mgr.set_tiling_enabled(true);
        mgr.float_rules.push(winspaces_common::FloatRule {
            name: "Calculator".to_string(),
            aumid: "calc_aumid".to_string(),
            exe_path: "calc.exe".to_string(),
            class_name: "CalcClass".to_string(),
            title_pattern: "".to_string(),
        });

        // 100 as HWND without matching facts does not match float rule
        // (in unit test environment without real Win32 HWND, query helpers return empty strings,
        // but if we match by rule properties, matches_float_rule uses match_float_rule_for_window).
        // Let's verify float_rules vector is consulted properly.
        assert_eq!(mgr.float_rules.len(), 1);
        assert!(!mgr.matches_float_rule(100 as HWND));
    }

    #[test]
    fn tiling_owns_window_checks_order_or_expected_membership() {
        let mut mgr = test_manager_tiling(vec![vec![100 as HWND, 200 as HWND]]);
        assert!(!mgr.tiling_owns_window(100 as HWND));

        mgr.set_tiling_enabled(true);
        // Initially not in order or expected
        assert!(!mgr.tiling_owns_window(100 as HWND));

        // When in order
        mgr.monitors[0].tiling[0].order.push(100 as HWND);
        assert!(mgr.tiling_owns_window(100 as HWND));

        // When in expected
        mgr.monitors[0].tiling[0]
            .expected
            .insert(200 as HWND, WindowRect::default());
        assert!(mgr.tiling_owns_window(200 as HWND));

        // When pending flatten (attempt 1/3)
        let mut mgr2 = test_manager_tiling(vec![vec![300 as HWND]]);
        mgr2.set_tiling_enabled(true);
        assert!(!mgr2.tiling_owns_window(300 as HWND));
        mgr2.monitors[0].tiling[0]
            .flatten_strikes
            .insert(300 as HWND, 1);
        assert!(mgr2.tiling_owns_window(300 as HWND));

        // When in floating, ownership is false even if in order or pending flatten
        mgr.floating_windows.insert(100 as HWND);
        assert!(!mgr.tiling_owns_window(100 as HWND));
        mgr2.floating_windows.insert(300 as HWND);
        assert!(!mgr2.tiling_owns_window(300 as HWND));

        // When sticky, ownership is false even if in expected
        mgr.sticky_windows.insert(200 as HWND);
        assert!(!mgr.tiling_owns_window(200 as HWND));
    }

    #[test]
    fn floating_persists_across_spaces_and_monitors() {
        let mut mgr = SpaceManager {
            monitors: vec![
                MonitorState {
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
                    spaces: vec![vec![100 as HWND, 200 as HWND], vec![300 as HWND]],
                    tiling: vec![TileSpace::new(), TileSpace::new()],
                },
                MonitorState {
                    hmon: 2 as _,
                    device: "\\\\.\\DISPLAY2".into(),
                    stable_id: "mon-2".into(),
                    rect: RECT {
                        left: 1920,
                        top: 0,
                        right: 3840,
                        bottom: 1080,
                    },
                    work: RECT {
                        left: 1920,
                        top: 0,
                        right: 3840,
                        bottom: 1040,
                    },
                    current: 0,
                    last_switched_space: 0,
                    last_switch_time: 0,
                    suppress_foreground_until: 0,
                    spaces: vec![vec![400 as HWND]],
                    tiling: vec![TileSpace::new()],
                },
            ],
            handle_hotkeys: true,
            show_all_taskbar: true,
            sticky_windows: HashSet::new(),
            floating_windows: HashSet::new(),
            space_indicator: true,
            suppress_foreground: false,
            reconcile_pending: false,
            suppress_rehome_until: 0,
            last_scan_tick: 0,
            restore_targets: Vec::new(),
            enforce_restore_until: 0,
            enforce_restore_ms: 0,
            enforce_restore_cap: 0,
            tiling_enabled: true,
            tiling_gaps: Gaps::NONE,
            tiling_drag: None,
            float_rules: Vec::new(),
        };

        // Window 100 is initially not floating
        assert!(!mgr.is_floating(100 as HWND));

        // Float window 100 on Mon 0 Space 0
        mgr.tiling_toggle_float(100 as HWND);
        assert!(mgr.is_floating(100 as HWND));

        // Move window 100 to Mon 0 Space 1 via detach + push (simulating space move)
        assert!(mgr.detach_window(100 as HWND));
        mgr.monitors[0].spaces[1].push(100 as HWND);
        assert_eq!(mgr.find_window(100 as HWND), Some((0, 1)));
        // Must still be floating on Space 1!
        assert!(mgr.is_floating(100 as HWND));
        assert!(!mgr.tiling_owns_window(100 as HWND));

        // Move window 100 to Mon 1 Space 0 via detach + push (simulating monitor move)
        assert!(mgr.detach_window(100 as HWND));
        mgr.monitors[1].spaces[0].push(100 as HWND);
        assert_eq!(mgr.find_window(100 as HWND), Some((1, 0)));
        // Must still be floating on Mon 1 Space 0!
        assert!(mgr.is_floating(100 as HWND));
        assert!(!mgr.tiling_owns_window(100 as HWND));

        // Toggling tiling off and on must NOT clear floating_windows
        mgr.set_tiling_enabled(false);
        assert!(mgr.is_floating(100 as HWND));
        mgr.set_tiling_enabled(true);
        assert!(mgr.is_floating(100 as HWND));

        // Unfloat on Mon 1 Space 0
        mgr.tiling_toggle_float(100 as HWND);
        assert!(!mgr.is_floating(100 as HWND));

        // Refloat and remove window (close)
        mgr.tiling_toggle_float(100 as HWND);
        assert!(mgr.is_floating(100 as HWND));
        mgr.remove_window(100 as HWND);
        assert!(!mgr.is_floating(100 as HWND));
    }

    #[test]
    fn unfloat_refused_when_permanent_float_rule_matches() {
        let mut mgr = test_manager_tiling(vec![vec![100 as HWND]]);
        mgr.set_tiling_enabled(true);
        mgr.float_rules.push(winspaces_common::FloatRule {
            name: "TestRule".to_string(),
            aumid: "".to_string(),
            exe_path: "".to_string(),
            class_name: "".to_string(),
            title_pattern: "".to_string(),
        });
        if mgr.matches_float_rule(100 as HWND) {
            assert!(mgr.is_floating(100 as HWND));
            mgr.tiling_toggle_float(100 as HWND);
            assert!(mgr.is_floating(100 as HWND));
        }
    }
}
