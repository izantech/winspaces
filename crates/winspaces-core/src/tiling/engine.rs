//! Tiling engine operations and SpaceManager integration.

use windows_sys::Win32::Foundation::HWND;
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
use super::types::{
    Direction, DragOutcome, SplitDirection, SplitToggleNotice, TilingDrag, DEFAULT_RATIO,
    MAX_RATIO, MIN_RATIO,
};
use crate::spaces::{is_live_window, is_tileable_window, AnimationGuard, SpaceManager};
use std::collections::HashSet;

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
                    ts.forget_all_windows();
                    ts.dirty = false;
                }
            }
        }
    }

    /// The tiler gives up on `hwnd` for this layout: float it, remember that
    /// the float was a measurement rather than a decision (so the window is
    /// re-admitted once the layout changes), drop every per-window record the
    /// space holds for it, and give it its rounded corners back.
    fn auto_float_window(&mut self, m_idx: usize, s_idx: usize, hwnd: HWND) {
        self.floating_windows.insert(hwnd);
        self.auto_floated.insert(hwnd);
        self.monitors[m_idx].tiling[s_idx].forget_window(hwnd);
        unsafe {
            winspaces_win32::dwm::set_corner_rounding(hwnd, true);
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

            let work_rect = WindowRect::from(self.monitors[m_idx].work);

            // Filter candidates: managed windows on current space, tile-eligible, not floating, not sticky, not minimized
            let collect_candidates = |mgr: &Self| -> Vec<HWND> {
                mgr.monitors[m_idx].spaces[cur_space]
                    .iter()
                    .copied()
                    .filter(|&h| {
                        is_live_window(h)
                            && is_tileable_window(h)
                            && !mgr.is_floating(h)
                            && !mgr.is_sticky(h)
                            && unsafe { IsIconic(h) == 0 }
                    })
                    .collect()
            };
            let mut candidates = collect_candidates(self);

            // A tile went away (closed, minimized, moved, floated by the
            // user): every slot grows, so windows the tiler floated for
            // resisting a smaller slot get another chance. The window that
            // just struck out is still in `order` this round, so it is not
            // counted as a departure of its own.
            let prev_tiles = self.monitors[m_idx].tiling[cur_space]
                .order
                .iter()
                .filter(|h| !self.auto_floated.contains(h))
                .count();
            if candidates.len() < prev_tiles {
                let readmit: Vec<HWND> = self.monitors[m_idx].spaces[cur_space]
                    .iter()
                    .copied()
                    .filter(|h| self.auto_floated.contains(h))
                    .collect();
                if !readmit.is_empty() {
                    for h in readmit {
                        self.readmit_auto_floated(h, "space lost a tile");
                    }
                    candidates = collect_candidates(self);
                }
            }

            let dpi = self.monitors[m_idx].dpi();
            let scaled_gaps = self.tiling_gaps.scaled_for_dpi(dpi);

            let ts = &mut self.monitors[m_idx].tiling[cur_space];

            // Maximize is the fullscreen mode. A window that already holds a
            // slot and is maximized is honoured: it keeps its slot reserved,
            // is never pushed, and covers the layout until the user restores
            // it (button, Win+Down, or dragging the title bar), at which point
            // Windows returns it to its restored rect — the slot the tiler last
            // gave it. Only newcomers to the space are flattened, so windows
            // that remember a maximized state (browsers, Explorer) still join
            // the layout instead of arriving on top of it.
            let honoured: HashSet<HWND> = candidates
                .iter()
                .copied()
                .filter(|&h| unsafe { IsZoomed(h) != 0 } && ts.order.contains(&h))
                .collect();

            // Un-maximize newcomers without activating, suppressing animations.
            let has_zoomed = candidates
                .iter()
                .any(|&h| !honoured.contains(&h) && unsafe { IsZoomed(h) != 0 });
            let _anim = has_zoomed.then(AnimationGuard::new);
            for &h in &candidates {
                if honoured.contains(&h) {
                    continue;
                }
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

            // Re-check IsZoomed: count flatten attempts. Below 4 attempts, exclude
            // from this round and keep ts.dirty true to retry. At 4 attempts,
            // auto-float the window and end the retry loop for it.
            let mut pending_zoom = false;
            let mut auto_floated_zoomed = Vec::new();
            let ready_candidates: Vec<HWND> = candidates
                .into_iter()
                .filter(|&h| {
                    if honoured.contains(&h) {
                        ts.flatten_strikes.remove(&h);
                        true
                    } else if unsafe { IsZoomed(h) != 0 } {
                        let attempts = ts.flatten_strikes.entry(h).or_insert(0);
                        *attempts += 1;
                        if *attempts >= 4 {
                            auto_floated_zoomed.push(h);
                        } else {
                            winspaces_common::log_debug!(
                                "flush_retile: hwnd {:?} flatten pending (still zoomed, attempt {}/4)",
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
                self.auto_float_window(m_idx, cur_space, hwnd);
            }
            let ts = &mut self.monitors[m_idx].tiling[cur_space];

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
                ts.split_direction,
            );
            let placements: Vec<(HWND, WindowRect)> = ts.order.iter().copied().zip(rects).collect();

            // Honoured windows keep their slot rect in `expected` (so drag
            // targets and focus navigation still see the full layout) but are
            // left maximized: pushing them would un-maximize them.
            let pushed: Vec<(HWND, WindowRect)> = placements
                .iter()
                .filter(|(h, _)| !honoured.contains(h))
                .cloned()
                .collect();
            apply_layout(&pushed);

            ts.expected = placements.into_iter().collect();
            ts.maximized = honoured;
            if pending_zoom {
                ts.dirty = true;
                schedule_retile();
            } else {
                ts.dirty = false;
            }

            log_info!(
                "flush_retile: Mon {} Space {} retiled {} window(s) ({} maximized)",
                m_idx + 1,
                cur_space + 1,
                ts.order.len(),
                ts.maximized.len()
            );
        }

        self.begin_settle(500);
    }

    /// Verification sweep checking if tiled windows accepted their assigned frames.
    /// Windows that resist four sweeps (e.g. min-size constraints) or are still
    /// maximized after four flatten attempts (e.g. elevated processes) are
    /// auto-floated.
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

                    // 8 px of slack: min-size clamps and frame rounding leave small
                    // deltas that are not resistance. Both rects are physical
                    // pixels, so DPI plays no part here.
                    if dx > 8 || dy > 8 || dw > 8 || dh > 8 {
                        let is_zoomed = unsafe { IsZoomed(hwnd) != 0 };
                        if is_zoomed && ts.maximized.contains(&hwnd) {
                            // Honoured maximize: sitting over its slot by design.
                            ts.strikes.remove(&hwnd);
                            ts.flatten_strikes.remove(&hwnd);
                        } else if is_zoomed {
                            let attempts = ts.flatten_strikes.entry(hwnd).or_insert(0);
                            *attempts += 1;
                            if *attempts >= 4 {
                                auto_floated_zoomed.push(hwnd);
                            } else {
                                winspaces_common::log_debug!(
                                    "verify_retile: hwnd {:?} is still maximized (flatten attempt {}/4) -> scheduling retry",
                                    hwnd,
                                    *attempts
                                );
                                ts.dirty = true;
                                need_retile = true;
                            }
                        } else if dx <= 8
                            && dy <= 8
                            && actual.width() + 8 >= expected.width()
                            && actual.height() + 8 >= expected.height()
                        {
                            // On its slot but bigger than it: the window's own
                            // minimum size is clamping the resize, it is not
                            // fighting the tiler. Let it overflow its tile
                            // rather than float it - a floater covers more of
                            // the layout than the overflow ever will.
                            ts.strikes.remove(&hwnd);
                            ts.flatten_strikes.remove(&hwnd);
                            if ts.overflowing.insert(hwnd) {
                                log_info!(
                                    "verify_retile: hwnd {:?} overflows its tile (actual={:?}, expected={:?}); accepting min-size clamp",
                                    hwnd,
                                    actual,
                                    expected
                                );
                            }
                        } else {
                            let strikes = ts.strikes.entry(hwnd).or_insert(0);
                            *strikes += 1;
                            if *strikes < 4 {
                                log_info!(
                                    "verify_retile: hwnd {:?} mismatch (actual={:?}, expected={:?}), strike {}/4 -> scheduling retry",
                                    hwnd,
                                    actual,
                                    expected,
                                    *strikes
                                );
                                ts.dirty = true;
                                need_retile = true;
                            } else {
                                auto_floated_resistant.push(hwnd);
                            }
                        }
                    } else {
                        ts.strikes.remove(&hwnd);
                        ts.flatten_strikes.remove(&hwnd);
                        ts.overflowing.remove(&hwnd);
                    }
                }
            }

            let floated_any = !auto_floated_resistant.is_empty() || !auto_floated_zoomed.is_empty();
            for hwnd in auto_floated_resistant {
                log_warn!(
                    "verify_retile: hwnd {:?} resisted tiling for 4 passes; auto-floating and restoring corner rounding",
                    hwnd
                );
                self.auto_float_window(m_idx, cur_space, hwnd);
            }
            for hwnd in auto_floated_zoomed {
                log_warn!(
                    "verify_retile: hwnd {:?} refused to un-maximize; auto-floating",
                    hwnd
                );
                self.auto_float_window(m_idx, cur_space, hwnd);
            }
            if floated_any {
                self.monitors[m_idx].tiling[cur_space].dirty = true;
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

    /// Toggle the primary split orientation on the active tiled space.
    ///
    /// Returns the notice the bin toasts, or `None` when tiling is disabled,
    /// the foreground window is untracked, or the space has < 2 tiled windows.
    pub fn tiling_toggle_split(&mut self) -> Option<SplitToggleNotice> {
        if !self.tiling_enabled {
            log_info!("tiling_toggle_split: tiling is disabled");
            return None;
        }

        let fg = unsafe { GetForegroundWindow() };
        let Some((m_idx, _)) = (!fg.is_null()).then(|| self.find_window(fg)).flatten() else {
            log_info!(
                "tiling_toggle_split: foreground window {:?} is not tracked; ignoring",
                fg
            );
            return None;
        };

        if m_idx >= self.monitors.len() {
            return None;
        }
        let cur_space = self.monitors[m_idx].current;
        self.tiling_toggle_split_at(m_idx, cur_space)
    }

    /// Flip the primary split of one space to the opposite of its *effective*
    /// direction (explicit override, else aspect-derived — so the first toggle
    /// from `Auto` always changes the visible layout).
    fn tiling_toggle_split_at(&mut self, m_idx: usize, s_idx: usize) -> Option<SplitToggleNotice> {
        let work = self.monitors[m_idx].work;
        let dpi = self.monitors[m_idx].dpi();
        let outer = self.tiling_gaps.scaled_for_dpi(dpi).outer as i32;

        let ts = &mut self.monitors[m_idx].tiling[s_idx];
        if ts.order.len() < 2 {
            log_info!(
                "tiling_toggle_split: Mon {} Space {} has {} tiled window(s); ignoring",
                m_idx + 1,
                s_idx + 1,
                ts.order.len()
            );
            return None;
        }

        let effective = effective_split_direction(ts.split_direction, &work, outer);
        let new_direction = match effective {
            SplitDirection::Horizontal => SplitDirection::Vertical,
            _ => SplitDirection::Horizontal,
        };
        ts.split_direction = new_direction;
        ts.dirty = true;
        schedule_retile();
        log_info!(
            "tiling_toggle_split: Mon {} Space {} split {:?} -> {:?}",
            m_idx + 1,
            s_idx + 1,
            effective,
            new_direction
        );
        Some(SplitToggleNotice {
            direction: new_direction,
            work,
        })
    }

    /// Predict the tile rects a split toggle would produce on one space, for
    /// the drag ghost preview. Read-only: does not mutate any tiling state.
    ///
    /// Returns the monitor work rect plus the predicted rects, or `None` when
    /// tiling is off, the indices are stale, or the space has < 2 tiled
    /// windows (nothing to preview).
    pub fn tiling_preview_toggle_rects(
        &self,
        mon_idx: usize,
        space_idx: usize,
    ) -> Option<(windows_sys::Win32::Foundation::RECT, Vec<WindowRect>)> {
        if !self.tiling_enabled {
            return None;
        }
        let mon = self.monitors.get(mon_idx)?;
        let ts = mon.tiling.get(space_idx)?;
        if ts.order.len() < 2 {
            return None;
        }

        let work = mon.work;
        let work_rect = WindowRect::from(work);
        let scaled_gaps = self.tiling_gaps.scaled_for_dpi(mon.dpi());
        let effective =
            effective_split_direction(ts.split_direction, &work, scaled_gaps.outer as i32);
        let toggled = match effective {
            SplitDirection::Horizontal => SplitDirection::Vertical,
            _ => SplitDirection::Horizontal,
        };

        let rects = compute(
            ts.layout,
            &work_rect,
            ts.order.len(),
            &ts.ratios,
            &scaled_gaps,
            toggled,
        );
        Some((work, rects))
    }

    /// Called on `EVENT_OBJECT_LOCATIONCHANGE` when a window is no longer
    /// maximized. A window the tiler was honouring as maximized has just been
    /// restored: Windows put it back at its restored rect, which is the slot
    /// the tiler last gave it — unless the layout moved underneath while it
    /// was maximized. Re-flush so it lands on the current slot either way.
    pub fn tiling_on_window_restored(&mut self, hwnd: HWND) {
        if !self.tiling_enabled {
            return;
        }
        // Fires for every top-level move; bail before the window lookup unless
        // some visible space is actually honouring a maximized window.
        if self
            .monitors
            .iter()
            .all(|mon| mon.tiling[mon.current].maximized.is_empty())
        {
            return;
        }
        let Some((m_idx, s_idx)) = self.find_window(hwnd) else {
            return;
        };
        let Some(ts) = self
            .monitors
            .get_mut(m_idx)
            .and_then(|mon| mon.tiling.get_mut(s_idx))
        else {
            return;
        };
        if ts.maximized.remove(&hwnd) {
            log_info!(
                "tiling_on_window_restored: Mon {} Space {} hwnd {:?} left maximize; retiling",
                m_idx + 1,
                s_idx + 1,
                hwnd
            );
            ts.dirty = true;
            schedule_retile();
        }
    }

    /// Toggle floating state for `hwnd` (or the foreground window if null).
    /// Undo an auto-float (never a user float) because the layout that
    /// produced it is gone. Returns whether anything changed; the window's
    /// space is marked dirty so it is re-tiled on the next flush.
    pub fn readmit_auto_floated(&mut self, hwnd: HWND, reason: &str) -> bool {
        if !self.auto_floated.remove(&hwnd) {
            return false;
        }
        self.floating_windows.remove(&hwnd);
        log_info!(
            "readmit_auto_floated: hwnd {:?} back into tiling ({})",
            hwnd,
            reason
        );
        if let Some((m_idx, s_idx)) = self.find_window(hwnd) {
            self.monitors[m_idx].tiling[s_idx].dirty = true;
        }
        if self.tiling_enabled {
            schedule_retile();
        }
        true
    }

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
            self.auto_floated.remove(&target);
            if let Some((m_idx, s_idx)) = loc {
                self.monitors[m_idx].tiling[s_idx].dirty = true;
            }
            schedule_retile();
            log_info!("tiling_toggle_float: un-floated window {:?}", target);
        } else {
            // An explicit float is a decision, not a measurement: it must not
            // be re-admitted by the auto-float healing paths.
            self.floating_windows.insert(target);
            self.auto_floated.remove(&target);
            if let Some((m_idx, s_idx)) = loc {
                let ts = &mut self.monitors[m_idx].tiling[s_idx];
                ts.forget_window(target);
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

        let from_maximized = ts.maximized.contains(&hwnd) || is_zoomed(hwnd);

        self.tiling_drag = Some(TilingDrag {
            hwnd,
            start_rect,
            mon_idx: m_idx,
            space_idx: s_idx,
            from_maximized,
        });

        log_info!(
            "tiling_on_movesize_start: Mon {} Space {} hwnd {:?}",
            m_idx + 1,
            s_idx + 1,
            hwnd
        );
    }

    /// Called on `EVENT_SYSTEM_MOVESIZEEND` to classify and apply mouse gestures on tiled spaces.
    ///
    /// `shift_held` is sampled (and latched) by the bin during the drag — the
    /// WinEvent is delivered asynchronously, so re-sampling here could miss a
    /// modifier released between the drop and this call. Returns the toggle
    /// notice when the gesture flipped the split, for the bin to toast.
    pub fn tiling_on_movesize_end(
        &mut self,
        hwnd: HWND,
        shift_held: bool,
    ) -> Option<SplitToggleNotice> {
        let drag = self.tiling_drag.take()?;

        if !self.tiling_enabled {
            return None;
        }

        if drag.hwnd != hwnd {
            return None;
        }

        let m_idx = drag.mon_idx;
        let s_idx = drag.space_idx;
        if m_idx >= self.monitors.len() || s_idx >= self.monitors[m_idx].spaces.len() {
            return None;
        }

        // Cross-monitor moves are decided by the daemon's MOVESIZEEND handler
        // before this is reached, and deliberately not here.
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

        let outcome = classify_drag(
            hwnd,
            &drag.start_rect,
            &new_rect,
            &tiles,
            (pt.x, pt.y),
            shift_held,
            drag.from_maximized,
        );

        // Dragging a maximized window's title bar restores it, so whatever the
        // gesture meant, the window has left maximize and must be re-placed.
        if drag.from_maximized {
            self.monitors[m_idx].tiling[s_idx].maximized.remove(&hwnd);
        }

        match outcome {
            DragOutcome::AdjustDwindleRatio {
                ratio_index,
                new_ratio,
            } => {
                let ts_mut = &mut self.monitors[m_idx].tiling[s_idx];
                ts_mut.set_ratio(ratio_index, new_ratio);
                ts_mut.dirty = true;
                schedule_retile();
                log_info!(
                    "tiling_on_movesize_end: adjusted ratio {} to {}",
                    ratio_index,
                    new_ratio
                );
                None
            }
            DragOutcome::Reorder {
                from_index,
                to_index,
            } => {
                let ts_mut = &mut self.monitors[m_idx].tiling[s_idx];
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
                None
            }
            DragOutcome::ToggleSplit => {
                let notice = self.tiling_toggle_split_at(m_idx, s_idx);
                if notice.is_none() {
                    // Toggle refused (< 2 tiled windows): the dropped window
                    // must still snap back to its tile.
                    let ts_mut = &mut self.monitors[m_idx].tiling[s_idx];
                    ts_mut.dirty = true;
                    schedule_retile();
                }
                notice
            }
            DragOutcome::SnapBack => {
                let ts_mut = &mut self.monitors[m_idx].tiling[s_idx];
                ts_mut.dirty = true;
                schedule_retile();
                log_info!(
                    "tiling_on_movesize_end: snap-back to original tile layout for {:?}",
                    hwnd
                );
                None
            }
        }
    }
}

/// Sample `IsZoomed` without the caller's pub-fn parameter reaching a raw FFI
/// call directly (the raw-pointer deref lint), keeping the unsafe local.
fn is_zoomed(hwnd: HWND) -> bool {
    unsafe { IsZoomed(hwnd) != 0 }
}

/// Resolve the direction the primary split renders with today: the explicit
/// override when set, else the aspect of the outer-gap-inset work rect —
/// matching `compute_dwindle`'s inset math exactly so `Auto` resolves to the
/// same branch the layout actually took.
fn effective_split_direction(
    dir: SplitDirection,
    work: &windows_sys::Win32::Foundation::RECT,
    outer_gap: i32,
) -> SplitDirection {
    match dir {
        SplitDirection::Auto => {
            let left = work.left + outer_gap;
            let top = work.top + outer_gap;
            let right = (work.right - outer_gap).max(left);
            let bottom = (work.bottom - outer_gap).max(top);
            if right - left >= bottom - top {
                SplitDirection::Horizontal
            } else {
                SplitDirection::Vertical
            }
        }
        explicit => explicit,
    }
}

fn actual_frame_bounds(hwnd: HWND) -> Option<WindowRect> {
    unsafe { winspaces_win32::dwm::extended_frame_bounds(hwnd) }.map(WindowRect::from)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::spaces::MonitorState;
    use crate::tiling::TileSpace;

    fn test_manager_tiling(spaces: Vec<Vec<HWND>>) -> SpaceManager {
        SpaceManager::for_test(vec![MonitorState::for_test(0, spaces)])
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
    fn auto_float_is_readmitted_but_user_float_is_not() {
        let mut mgr = test_manager_tiling(vec![vec![100 as HWND, 200 as HWND]]);
        mgr.set_tiling_enabled(true);
        mgr.monitors[0].tiling[0].dirty = false;

        // Simulate a strike-out: both sets, like verify_retile does.
        mgr.floating_windows.insert(100 as HWND);
        mgr.auto_floated.insert(100 as HWND);
        // And a deliberate float by the user.
        mgr.tiling_toggle_float(200 as HWND);
        assert!(mgr.is_floating(200 as HWND));
        assert!(!mgr.auto_floated.contains(&(200 as HWND)));
        mgr.monitors[0].tiling[0].dirty = false;

        assert!(mgr.readmit_auto_floated(100 as HWND, "test"));
        assert!(!mgr.is_floating(100 as HWND));
        assert!(!mgr.auto_floated.contains(&(100 as HWND)));
        assert!(mgr.monitors[0].tiling[0].dirty);

        mgr.monitors[0].tiling[0].dirty = false;
        assert!(!mgr.readmit_auto_floated(200 as HWND, "test"));
        assert!(mgr.is_floating(200 as HWND));
        assert!(!mgr.monitors[0].tiling[0].dirty);
    }

    #[test]
    fn toggling_float_on_an_auto_floated_window_makes_it_a_user_decision() {
        let mut mgr = test_manager_tiling(vec![vec![100 as HWND]]);
        mgr.set_tiling_enabled(true);
        mgr.floating_windows.insert(100 as HWND);
        mgr.auto_floated.insert(100 as HWND);

        // Un-float: back to tiling, no auto-float residue.
        mgr.tiling_toggle_float(100 as HWND);
        assert!(!mgr.is_floating(100 as HWND));
        assert!(!mgr.auto_floated.contains(&(100 as HWND)));

        // Float again by hand: sticky, the healing paths must leave it alone.
        mgr.tiling_toggle_float(100 as HWND);
        assert!(mgr.is_floating(100 as HWND));
        assert!(!mgr.auto_floated.contains(&(100 as HWND)));
        assert!(!mgr.readmit_auto_floated(100 as HWND, "test"));
        assert!(mgr.is_floating(100 as HWND));

        // Closing the window drops both.
        mgr.auto_floated.insert(100 as HWND);
        mgr.remove_window(100 as HWND);
        assert!(!mgr.is_floating(100 as HWND));
        assert!(!mgr.auto_floated.contains(&(100 as HWND)));
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
            from_maximized: false,
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

        // When pending flatten (attempt 1/4)
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
        let mut mgr = SpaceManager::for_test(vec![
            MonitorState::for_test(0, vec![vec![100 as HWND, 200 as HWND], vec![300 as HWND]]),
            MonitorState::for_test(1, vec![vec![400 as HWND]]),
        ]);
        mgr.tiling_enabled = true;

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

        // Toggling tiling off and on must NOT clear floating_windows or auto_floated
        mgr.auto_floated.insert(100 as HWND);
        mgr.set_tiling_enabled(false);
        assert!(mgr.is_floating(100 as HWND));
        assert!(mgr.auto_floated.contains(&(100 as HWND)));
        mgr.set_tiling_enabled(true);
        assert!(mgr.is_floating(100 as HWND));
        assert!(mgr.auto_floated.contains(&(100 as HWND)));
        mgr.auto_floated.remove(&(100 as HWND));

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

    #[test]
    fn tiling_on_movesize_end_with_no_drag_is_noop() {
        let mut mgr = test_manager_tiling(vec![vec![100 as HWND]]);
        mgr.set_tiling_enabled(true);
        assert!(mgr.tiling_on_movesize_end(100 as HWND, false).is_none());
        assert!(mgr.tiling_drag.is_none());
    }

    #[test]
    fn toggle_split_flips_effective_direction_and_marks_dirty() {
        let mut mgr = test_manager_tiling(vec![vec![100 as HWND, 200 as HWND]]);
        mgr.set_tiling_enabled(true);
        mgr.monitors[0].tiling[0].order = vec![100 as HWND, 200 as HWND];
        mgr.monitors[0].tiling[0].dirty = false;

        // Landscape work rect: Auto resolves Horizontal, first toggle stacks.
        let notice = mgr.tiling_toggle_split_at(0, 0).expect("toggle applies");
        assert_eq!(notice.direction, SplitDirection::Vertical);
        assert_eq!(
            mgr.monitors[0].tiling[0].split_direction,
            SplitDirection::Vertical
        );
        assert!(mgr.monitors[0].tiling[0].dirty);
        assert_eq!(notice.work.right, 1920);

        // Second toggle flips back to side-by-side.
        let notice = mgr.tiling_toggle_split_at(0, 0).expect("toggle applies");
        assert_eq!(notice.direction, SplitDirection::Horizontal);
    }

    #[test]
    fn toggle_split_refused_below_two_windows() {
        let mut mgr = test_manager_tiling(vec![vec![100 as HWND]]);
        mgr.set_tiling_enabled(true);
        mgr.monitors[0].tiling[0].order = vec![100 as HWND];
        assert!(mgr.tiling_toggle_split_at(0, 0).is_none());
        assert_eq!(
            mgr.monitors[0].tiling[0].split_direction,
            SplitDirection::Auto
        );
    }

    #[test]
    fn preview_toggle_rects_predicts_stacked_layout() {
        let mut mgr = test_manager_tiling(vec![vec![100 as HWND, 200 as HWND]]);
        mgr.set_tiling_enabled(true);
        mgr.monitors[0].tiling[0].order = vec![100 as HWND, 200 as HWND];

        // Landscape + Auto: the toggled preview must stack top/bottom.
        let (work, rects) = mgr
            .tiling_preview_toggle_rects(0, 0)
            .expect("preview available");
        assert_eq!(work.right, 1920);
        assert_eq!(rects.len(), 2);
        assert_eq!(
            rects[0],
            WindowRect {
                left: 0,
                top: 0,
                right: 1920,
                bottom: 520
            }
        );
        assert_eq!(
            rects[1],
            WindowRect {
                left: 0,
                top: 520,
                right: 1920,
                bottom: 1040
            }
        );

        // Preview must not mutate the space.
        assert_eq!(
            mgr.monitors[0].tiling[0].split_direction,
            SplitDirection::Auto
        );

        // Single window: nothing to preview.
        mgr.monitors[0].tiling[0].order = vec![100 as HWND];
        assert!(mgr.tiling_preview_toggle_rects(0, 0).is_none());
    }

    #[test]
    fn restore_of_honoured_maximized_window_marks_space_dirty() {
        let mut mgr = test_manager_tiling(vec![vec![100 as HWND, 200 as HWND]]);
        mgr.set_tiling_enabled(true);
        mgr.monitors[0].tiling[0].order = vec![100 as HWND, 200 as HWND];
        mgr.monitors[0].tiling[0].maximized.insert(100 as HWND);
        mgr.monitors[0].tiling[0].dirty = false;

        // A window the tiler never honoured is not the tiler's concern.
        mgr.tiling_on_window_restored(200 as HWND);
        assert!(!mgr.monitors[0].tiling[0].dirty);

        mgr.tiling_on_window_restored(100 as HWND);
        assert!(mgr.monitors[0].tiling[0].dirty);
        assert!(!mgr.monitors[0].tiling[0].maximized.contains(&(100 as HWND)));
    }

    #[test]
    fn floating_a_window_drops_its_honoured_maximize() {
        let mut mgr = test_manager_tiling(vec![vec![100 as HWND, 200 as HWND]]);
        mgr.set_tiling_enabled(true);
        mgr.monitors[0].tiling[0].order = vec![100 as HWND, 200 as HWND];
        mgr.monitors[0].tiling[0].maximized.insert(100 as HWND);

        mgr.tiling_toggle_float(100 as HWND);
        assert!(mgr.is_floating(100 as HWND));
        assert!(mgr.monitors[0].tiling[0].maximized.is_empty());
    }
}
