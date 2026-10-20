//! Shelf: on a space with Shelf on, one window is on stage and every other
//! top-level window is hidden and shown as a card in a strip along one edge
//! of the monitor. `shelf_apply` re-derives the roles from membership (see
//! `resolve`) and commits them; visibility passes read the committed cards.
//! There is no separate set of hidden windows to drift from the space lists.

use std::ptr::null_mut;
use windows_sys::Win32::Foundation::{HWND, RECT};
use windows_sys::Win32::Graphics::Gdi::{MonitorFromWindow, MONITOR_DEFAULTTONEAREST};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    GetAncestor, GetForegroundWindow, GetTopWindow, GetWindow, GetWindowRect, IsIconic, IsZoomed,
    SetForegroundWindow, SetWindowPos, GA_ROOTOWNER, GW_HWNDNEXT, SWP_NOACTIVATE, SWP_NOZORDER,
};
use winspaces_common::layout::clamp_to_work;
use winspaces_common::{log_info, ShelfSide, WindowRect};

use super::eligibility::{is_tileable_window, is_valid_window};
use super::manager::SpaceManager;
use super::notify::request_shelf_refresh;
use super::state::{
    get_window_state, WINSPACES_STATE_CLOAKED, WINSPACES_STATE_HIDDEN_MASK,
    WINSPACES_STATE_SHELL_CLOAKED, WINSPACES_STATE_WAS_ICONIC,
};
use super::visibility::set_window_visibility;

/// Per-space Shelf state, parallel to `MonitorState::spaces`.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ShelfSpace {
    /// Shelf switched on for this space alone (`SpaceManager::shelf_everywhere`
    /// covers every space without touching this).
    pub on: bool,
    pub stage: Option<HWND>,
    /// The cards, most recent first.
    pub order: Vec<HWND>,
}

/// What `resolve` needs to know about one window of the space.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Member {
    pub hwnd: HWND,
    /// The root owner, for an owned window (dialogs, tool palettes).
    pub root: Option<HWND>,
    pub minimized: bool,
    pub sticky: bool,
}

/// Decide the stage and the cards from the last committed roles and the
/// space's current members.
///
/// - Owned and pinned windows are never on stage nor cards; owned ones follow
///   their root.
/// - A member that is neither on stage nor a card is new to the space (opened,
///   moved in, restored from minimized, unpinned) and takes the stage; the
///   previous stage becomes the most recent card.
/// - A minimized stage keeps the stage, empty, rather than pulling a card up.
/// - A stage that left the space hands it to the most recent card.
pub(crate) fn resolve(
    stage: Option<HWND>,
    order: &[HWND],
    members: &[Member],
) -> (Option<HWND>, Vec<HWND>) {
    let top_level = |m: &&Member| m.root.is_none() && !m.sticky;
    let can_stage = |h: HWND| members.iter().filter(top_level).any(|m| m.hwnd == h);
    let can_card = |h: HWND| {
        members
            .iter()
            .filter(top_level)
            .any(|m| m.hwnd == h && !m.minimized)
    };

    let mut stage = stage.filter(|&h| can_stage(h));
    let mut cards: Vec<HWND> = order
        .iter()
        .copied()
        .filter(|&h| can_card(h) && Some(h) != stage)
        .collect();
    let newcomers: Vec<HWND> = members
        .iter()
        .filter(top_level)
        .filter(|m| !m.minimized && Some(m.hwnd) != stage && !order.contains(&m.hwnd))
        .map(|m| m.hwnd)
        .collect();
    for h in newcomers {
        if let Some(old) = stage.filter(|&old| can_card(old)) {
            cards.insert(0, old);
        }
        stage = Some(h);
    }
    if stage.is_none() && !cards.is_empty() {
        stage = Some(cards.remove(0));
    }
    (stage, cards)
}

/// Whether `hwnd`, or the root that owns it, is one of `cards`.
pub(crate) fn shelved_by(cards: &[HWND], hwnd: HWND) -> bool {
    cards.contains(&hwnd) || root_owner(hwnd).is_some_and(|r| cards.contains(&r))
}

fn root_owner(hwnd: HWND) -> Option<HWND> {
    let root = unsafe { GetAncestor(hwnd, GA_ROOTOWNER) };
    (!root.is_null() && root != hwnd).then_some(root)
}

/// Minimized by the user. A window we hid with the forced-minimize backend is
/// iconic too, so a hidden window answers from the flag the hide recorded.
fn user_minimized(hwnd: HWND) -> bool {
    let state = get_window_state(hwnd);
    if state & WINSPACES_STATE_HIDDEN_MASK != 0 {
        state & WINSPACES_STATE_WAS_ICONIC != 0
    } else {
        unsafe { IsIconic(hwnd) != 0 }
    }
}

/// Maximized or covering its whole monitor (borderless fullscreen). The strip
/// steps aside for such a stage: maximize is the fullscreen mode.
#[allow(clippy::not_unsafe_ptr_arg_deref)]
pub fn fills_monitor(hwnd: HWND) -> bool {
    unsafe {
        if IsZoomed(hwnd) != 0 {
            return true;
        }
        let Some(mon) = winspaces_win32::display::monitor_rect_of(MonitorFromWindow(
            hwnd,
            MONITOR_DEFAULTTONEAREST,
        )) else {
            return false;
        };
        let mut r: RECT = std::mem::zeroed();
        GetWindowRect(hwnd, &mut r) != 0
            && r.left <= mon.left
            && r.top <= mon.top
            && r.right >= mon.right
            && r.bottom >= mon.bottom
    }
}

/// The strip and the stage area a work rect splits into for `side`.
pub(crate) fn split_work(work: &WindowRect, side: ShelfSide, dpi: u32) -> (WindowRect, WindowRect) {
    let base = if side.is_vertical() { 200 } else { 150 };
    let t = winspaces_win32::dpi::px(dpi as f32 / 96.0, base);
    let mut strip = work.clone();
    let mut stage = work.clone();
    match side {
        ShelfSide::Left => {
            strip.right = work.left + t;
            stage.left = strip.right;
        }
        ShelfSide::Right => {
            strip.left = work.right - t;
            stage.right = strip.left;
        }
        ShelfSide::Top => {
            strip.bottom = work.top + t;
            stage.top = strip.bottom;
        }
        ShelfSide::Bottom => {
            strip.top = work.bottom - t;
            stage.bottom = strip.top;
        }
    }
    (strip, stage)
}

impl SpaceManager {
    pub fn shelf_on(&self, mon_idx: usize, space_idx: usize) -> bool {
        self.shelf_everywhere
            || self
                .monitors
                .get(mon_idx)
                .and_then(|m| m.shelf.get(space_idx))
                .is_some_and(|s| s.on)
    }

    fn shelf_any(&self) -> bool {
        self.shelf_everywhere || self.monitors.iter().any(|m| m.shelf.iter().any(|s| s.on))
    }

    /// Wake the bin's Shelf refresh, but only while some Shelf is on: the
    /// triggers sit on hot paths a daemon without Shelf must not pay for.
    pub(crate) fn schedule_shelf(&self) {
        if self.shelf_any() {
            request_shelf_refresh();
        }
    }

    fn shelf_members(&self, mon_idx: usize, space_idx: usize) -> Vec<Member> {
        self.monitors[mon_idx].spaces[space_idx]
            .iter()
            .map(|&hwnd| Member {
                hwnd,
                root: root_owner(hwnd),
                minimized: user_minimized(hwnd),
                sticky: self.is_sticky(hwnd),
            })
            .collect()
    }

    /// The stage and cards of a space, resolved against its members right now.
    fn shelf_roles(&self, mon_idx: usize, space_idx: usize) -> (Option<HWND>, Vec<HWND>) {
        let entry = &self.monitors[mon_idx].shelf[space_idx];
        resolve(
            entry.stage,
            &entry.order,
            &self.shelf_members(mon_idx, space_idx),
        )
    }

    /// The committed cards of a space; none while Shelf is off.
    pub(crate) fn shelf_cards(&self, mon_idx: usize, space_idx: usize) -> &[HWND] {
        if self.shelf_on(mon_idx, space_idx) {
            &self.monitors[mon_idx].shelf[space_idx].order
        } else {
            &[]
        }
    }

    pub(crate) fn is_shelved(&self, mon_idx: usize, space_idx: usize, hwnd: HWND) -> bool {
        shelved_by(self.shelf_cards(mon_idx, space_idx), hwnd)
    }

    /// The strip and stage area of a monitor whose current space has Shelf on.
    fn shelf_split(&self, mon_idx: usize) -> Option<(WindowRect, WindowRect)> {
        let mon = &self.monitors[mon_idx];
        self.shelf_on(mon_idx, mon.current)
            .then(|| split_work(&WindowRect::from(mon.work), self.shelf_side, mon.dpi()))
    }

    /// The monitor's work area, minus the strip when its current space has
    /// Shelf on.
    pub fn stage_area(&self, mon_idx: usize) -> WindowRect {
        self.shelf_split(mon_idx).map_or_else(
            || WindowRect::from(self.monitors[mon_idx].work),
            |(_, stage)| stage,
        )
    }

    /// The committed stage of a monitor's current space, while Shelf is on.
    pub fn shelf_stage_of(&self, mon_idx: usize) -> Option<HWND> {
        let mon = &self.monitors[mon_idx];
        self.shelf_on(mon_idx, mon.current)
            .then(|| mon.shelf[mon.current].stage)
            .flatten()
    }

    /// What a display's strip shows: its rect and the committed cards, each
    /// with whether a live thumbnail is possible (a cloaked window keeps its
    /// surface; a minimized or `SW_HIDE`-hidden one does not). `None` when the
    /// current space has Shelf off or no cards.
    pub fn shelf_strip(&self, mon_idx: usize) -> Option<(WindowRect, Vec<(HWND, bool)>)> {
        let (strip, _) = self.shelf_split(mon_idx)?;
        let mon = &self.monitors[mon_idx];
        let order = &mon.shelf[mon.current].order;
        if order.is_empty() {
            return None;
        }
        let live = WINSPACES_STATE_CLOAKED | WINSPACES_STATE_SHELL_CLOAKED;
        let cards = order
            .iter()
            .map(|&h| (h, get_window_state(h) & live != 0))
            .collect();
        Some((strip, cards))
    }

    /// Flip Shelf on the focused display's current space, or on every space
    /// when `everywhere`. Returns the new state.
    pub fn toggle_shelf(&mut self, mon_idx: usize, everywhere: bool) -> bool {
        let Some(mon) = self.monitors.get(mon_idx) else {
            return false;
        };
        let next = !self.shelf_on(mon_idx, mon.current);
        if everywhere {
            self.set_shelf_everywhere(next);
        } else {
            self.set_shelf_on(mon_idx, mon.current, next);
        }
        next
    }

    fn set_shelf_on(&mut self, mon_idx: usize, space_idx: usize, on: bool) {
        let Some(entry) = self
            .monitors
            .get(mon_idx)
            .and_then(|m| m.shelf.get(space_idx))
        else {
            return;
        };
        if entry.on == on {
            return;
        }
        let was = self.shelf_on(mon_idx, space_idx);
        self.monitors[mon_idx].shelf[space_idx].on = on;
        self.shelf_changed(mon_idx, space_idx, was);
        request_shelf_refresh();
    }

    /// Shelf on every space of every display. Clears the per-space toggles
    /// either way: on, they are covered; off means off everywhere.
    pub fn set_shelf_everywhere(&mut self, on: bool) {
        if self.shelf_everywhere == on {
            return;
        }
        let was: Vec<Vec<bool>> = (0..self.monitors.len())
            .map(|m| {
                (0..self.monitors[m].shelf.len())
                    .map(|s| self.shelf_on(m, s))
                    .collect()
            })
            .collect();
        self.shelf_everywhere = on;
        for mon in &mut self.monitors {
            for entry in &mut mon.shelf {
                entry.on = false;
            }
        }
        for (m, spaces) in was.into_iter().enumerate() {
            for (s, was_on) in spaces.into_iter().enumerate() {
                self.shelf_changed(m, s, was_on);
            }
        }
        request_shelf_refresh();
    }

    pub fn set_shelf_side(&mut self, side: ShelfSide) {
        if self.shelf_side == side {
            return;
        }
        self.shelf_side = side;
        for m in 0..self.monitors.len() {
            let s = self.monitors[m].current;
            if self.shelf_on(m, s) {
                if let Some(stage) = self.monitors[m].shelf[s].stage {
                    self.fit_stage(m, stage);
                }
                self.mark_tiling_dirty(m, s);
            }
        }
        request_shelf_refresh();
    }

    /// Settle one space after its effective Shelf state may have changed:
    /// seed the roles when it came on, forget them and show every card when
    /// it went off.
    fn shelf_changed(&mut self, mon_idx: usize, space_idx: usize, was: bool) {
        let now = self.shelf_on(mon_idx, space_idx);
        if now == was {
            return;
        }
        if now {
            self.seed_shelf(mon_idx, space_idx);
        } else {
            let on = self.monitors[mon_idx].shelf[space_idx].on;
            self.monitors[mon_idx].shelf[space_idx] = ShelfSpace {
                on,
                ..Default::default()
            };
        }
        log_info!(
            "Shelf {} on Mon {}, Space {}",
            if now { "on" } else { "off" },
            mon_idx + 1,
            space_idx + 1
        );
        if self.monitors[mon_idx].current == space_idx {
            self.sync_shelf_visibility(mon_idx, space_idx);
        }
        self.mark_tiling_dirty(mon_idx, space_idx);
    }

    /// The topmost top-level window takes the stage and the rest become
    /// cards in z-order. Z-order rather than the foreground, because the
    /// toggle can come from Overview, which holds the foreground itself.
    /// Seeding `order` with every window is what keeps `resolve` from
    /// reading them as newcomers.
    fn seed_shelf(&mut self, mon_idx: usize, space_idx: usize) {
        let candidates: Vec<HWND> = self
            .shelf_members(mon_idx, space_idx)
            .into_iter()
            .filter(|m| m.root.is_none() && !m.sticky && !m.minimized)
            .map(|m| m.hwnd)
            .collect();
        let mut order = Vec::with_capacity(candidates.len());
        unsafe {
            let mut h = GetTopWindow(null_mut());
            while !h.is_null() && order.len() < candidates.len() {
                if candidates.contains(&h) {
                    order.push(h);
                }
                h = GetWindow(h, GW_HWNDNEXT);
            }
        }
        // Hidden windows of a background space have no meaningful z-order.
        let rest: Vec<HWND> = candidates
            .iter()
            .rev()
            .copied()
            .filter(|h| !order.contains(h))
            .collect();
        order.extend(rest);
        let stage = (!order.is_empty()).then(|| order.remove(0));
        let entry = &mut self.monitors[mon_idx].shelf[space_idx];
        entry.stage = stage;
        entry.order = order;
    }

    /// Show or hide each window of a showing space to match `should_be_visible`.
    fn sync_shelf_visibility(&mut self, mon_idx: usize, space_idx: usize) {
        let show_all = self.show_all_taskbar;
        for hwnd in self.monitors[mon_idx].spaces[space_idx].clone() {
            if !is_valid_window(hwnd) {
                continue;
            }
            let hidden = get_window_state(hwnd) & WINSPACES_STATE_HIDDEN_MASK != 0;
            let visible = self.should_be_visible(mon_idx, space_idx, hwnd);
            if hidden == visible {
                set_window_visibility(hwnd, visible, show_all);
            }
        }
    }

    /// Commit the roles of every showing Shelf space and make the screen agree:
    /// a newcomer takes the stage, a closed stage hands it to the next card.
    pub fn shelf_apply(&mut self) {
        for m in 0..self.monitors.len() {
            let s = self.monitors[m].current;
            if !self.shelf_on(m, s) {
                continue;
            }
            let (stage, cards) = self.shelf_roles(m, s);
            let entry = &mut self.monitors[m].shelf[s];
            let moved = entry.stage != stage;
            entry.stage = stage;
            entry.order = cards;
            self.sync_shelf_visibility(m, s);
            if moved {
                if let Some(h) = stage {
                    self.fit_stage(m, h);
                    unsafe {
                        if GetForegroundWindow() != h && IsIconic(h) == 0 {
                            SetForegroundWindow(h);
                        }
                    }
                }
                self.mark_tiling_dirty(m, s);
            }
        }
    }

    /// Put `hwnd` on stage in place of the current stage window, which
    /// becomes the most recent card. Returns whether the roles changed;
    /// nothing is shown or hidden.
    pub(crate) fn shelf_stage(&mut self, mon_idx: usize, space_idx: usize, hwnd: HWND) -> bool {
        if !self.shelf_on(mon_idx, space_idx) {
            return false;
        }
        let (stage, mut cards) = self.shelf_roles(mon_idx, space_idx);
        if stage == Some(hwnd) || !cards.contains(&hwnd) {
            return false;
        }
        cards.retain(|&c| c != hwnd);
        if let Some(old) = stage.filter(|&old| !user_minimized(old)) {
            cards.insert(0, old);
        }
        let entry = &mut self.monitors[mon_idx].shelf[space_idx];
        entry.stage = Some(hwnd);
        entry.order = cards;
        true
    }

    /// A card was clicked or its window activated: swap it onto the stage.
    /// The new stage is shown and focused before the old one is hidden, so
    /// the OS never has to pick a window to activate in between.
    #[allow(clippy::not_unsafe_ptr_arg_deref)]
    pub fn shelf_promote(&mut self, hwnd: HWND) {
        let Some((m, s)) = self.find_window(hwnd) else {
            return;
        };
        if !self.shelf_stage(m, s, hwnd) {
            return;
        }
        log_info!(
            "Shelf: hwnd {:?} on stage on Mon {}, Space {}",
            hwnd,
            m + 1,
            s + 1
        );
        if self.monitors[m].current == s {
            set_window_visibility(hwnd, true, self.show_all_taskbar);
            self.fit_stage(m, hwnd);
            unsafe {
                SetForegroundWindow(hwnd);
            }
            self.sync_shelf_visibility(m, s);
        }
        self.mark_tiling_dirty(m, s);
    }

    /// Reorder the cards of a monitor's current space. Cards missing from
    /// `order` (past the end of the strip) keep their relative order after it.
    pub fn shelf_reorder(&mut self, mon_idx: usize, order: &[HWND]) {
        let Some(mon) = self.monitors.get_mut(mon_idx) else {
            return;
        };
        let entry = &mut mon.shelf[mon.current];
        let mut next: Vec<HWND> = order
            .iter()
            .copied()
            .filter(|h| entry.order.contains(h))
            .collect();
        next.extend(entry.order.iter().copied().filter(|h| !order.contains(h)));
        entry.order = next;
        request_shelf_refresh();
    }

    /// Move a window entering the stage out from under the strip. Only on
    /// entry: the user can still drag it over the strip afterwards. Maximized
    /// windows are left alone (maximize is fullscreen) and tiled ones belong
    /// to the tiler.
    fn fit_stage(&self, mon_idx: usize, hwnd: HWND) {
        unsafe {
            if IsZoomed(hwnd) != 0 || IsIconic(hwnd) != 0 {
                return;
            }
        }
        if self.tiling_enabled && !self.is_floating(hwnd) && is_tileable_window(hwnd) {
            return;
        }
        let Some(frame) = (unsafe { winspaces_win32::dwm::extended_frame_bounds(hwnd) }) else {
            return;
        };
        let frame = WindowRect::from(frame);
        let target = clamp_to_work(&frame, &self.stage_area(mon_idx));
        if target == frame {
            return;
        }
        unsafe {
            let (l, t, r, b) = winspaces_win32::dwm::dwm_shadow_margins(hwnd);
            SetWindowPos(
                hwnd,
                null_mut(),
                target.left - l,
                target.top - t,
                target.width() + l + r,
                target.height() + t + b,
                SWP_NOZORDER | SWP_NOACTIVATE,
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const A: HWND = 1 as HWND;
    const B: HWND = 2 as HWND;
    const C: HWND = 3 as HWND;
    const D: HWND = 4 as HWND;

    fn top(hwnd: HWND) -> Member {
        Member {
            hwnd,
            root: None,
            minimized: false,
            sticky: false,
        }
    }

    #[test]
    fn settled_roles_are_kept() {
        let members = [top(A), top(B), top(C)];
        assert_eq!(resolve(Some(A), &[B, C], &members), (Some(A), vec![B, C]));
    }

    #[test]
    fn a_newcomer_takes_the_stage() {
        let members = [top(A), top(B), top(C)];
        assert_eq!(resolve(Some(A), &[B], &members), (Some(C), vec![A, B]));
    }

    #[test]
    fn a_vacated_stage_goes_to_the_most_recent_card() {
        let members = [top(B), top(C)];
        assert_eq!(resolve(Some(A), &[C, B], &members), (Some(C), vec![B]));
    }

    #[test]
    fn a_minimized_stage_stays_and_pulls_nothing_up() {
        let mut a = top(A);
        a.minimized = true;
        let members = [a, top(B)];
        assert_eq!(resolve(Some(A), &[B], &members), (Some(A), vec![B]));
    }

    #[test]
    fn a_newcomer_drops_a_minimized_stage_from_the_cards() {
        let mut a = top(A);
        a.minimized = true;
        let members = [a, top(B), top(C)];
        assert_eq!(resolve(Some(A), &[B], &members), (Some(C), vec![B]));
    }

    #[test]
    fn owned_pinned_and_minimized_windows_are_never_cards() {
        let mut b = top(B);
        b.root = Some(A);
        let mut c = top(C);
        c.sticky = true;
        let mut d = top(D);
        d.minimized = true;
        let members = [top(A), b, c, d];
        assert_eq!(resolve(Some(A), &[], &members), (Some(A), vec![]));
    }

    #[test]
    fn stale_cards_are_dropped() {
        let members = [top(A), top(C)];
        assert_eq!(resolve(Some(A), &[B, C], &members), (Some(A), vec![C]));
    }

    #[test]
    fn an_empty_stage_is_filled_from_the_cards() {
        let members = [top(A), top(B)];
        assert_eq!(resolve(None, &[B, A], &members), (Some(B), vec![A]));
    }

    #[test]
    fn the_strip_and_stage_tile_the_work_area() {
        let work = WindowRect {
            left: 0,
            top: 0,
            right: 1920,
            bottom: 1040,
        };
        let (strip, stage) = split_work(&work, ShelfSide::Left, 96);
        assert_eq!((strip.left, strip.right, stage.left), (0, 200, 200));
        let (strip, stage) = split_work(&work, ShelfSide::Right, 144);
        assert_eq!((strip.left, stage.right), (1620, 1620));
        let (strip, stage) = split_work(&work, ShelfSide::Top, 96);
        assert_eq!((strip.bottom, stage.top, stage.bottom), (150, 150, 1040));
        let (strip, stage) = split_work(&work, ShelfSide::Bottom, 96);
        assert_eq!((strip.top, stage.bottom, strip.bottom), (890, 890, 1040));
    }
}
