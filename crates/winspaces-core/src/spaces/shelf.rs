//! Shelf: on a space with Shelf on, a set of windows is on stage and every
//! other top-level window is hidden and shown as a card in a strip along one
//! edge of the monitor. A card is itself a set: the windows that were on
//! stage together leave and come back together. `shelf_apply` re-derives the
//! roles from membership (see `resolve`) and commits them; visibility passes
//! read the committed cards. There is no separate set of hidden windows to
//! drift from the space lists.

use std::ptr::null_mut;
use windows_sys::Win32::Foundation::{HWND, POINT, RECT};
use windows_sys::Win32::Graphics::Gdi::{MonitorFromWindow, MONITOR_DEFAULTTONEAREST};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    GetTopWindow, GetWindow, GetWindowRect, GetWindowThreadProcessId, IsIconic, IsZoomed,
    SetForegroundWindow, SetWindowPos, GW_HWNDNEXT, SWP_NOACTIVATE, SWP_NOZORDER,
};
use winspaces_common::layout::clamp_to_work;
use winspaces_common::{log_info, ShelfSide, WindowRect};

use super::eligibility::{
    app_root, foreground_root, is_framed_window, is_tileable_window, is_valid_window,
};
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
    /// The windows on stage, topmost first.
    pub stage: Vec<HWND>,
    /// The cards, most recent first; each is a set of windows, topmost first.
    pub order: Vec<Vec<HWND>>,
}

/// The cards a strip shows: each a set of windows, topmost first, with
/// whether its thumbnail is live.
pub type StripCards = Vec<Vec<(HWND, bool)>>;

/// What `resolve` needs to know about one window of the space.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Member {
    pub hwnd: HWND,
    /// The app window it belongs to, for an owned window (dialogs, palettes).
    pub root: Option<HWND>,
    pub minimized: bool,
    pub sticky: bool,
    pub pid: u32,
    /// Has a caption bar; a frameless window is an app's viewer or overlay.
    pub framed: bool,
}

/// Decide the stage and the cards from the last committed roles and the
/// space's current members.
///
/// - Owned and pinned windows are never on stage nor cards; owned ones follow
///   their root.
/// - A member that is neither on stage nor in a card is new to the space
///   (opened, moved in, restored from a card, unpinned) and takes the stage
///   alone; the previous stage set becomes the most recent card. A frameless
///   newcomer of the same process as a stage member joins the stage set
///   instead (Telegram's media viewer).
/// - A minimized stage member stays in the stage set, so restoring it brings
///   it back next to its set. A stage with only minimized members stays,
///   empty, rather than pulling a card up.
/// - A minimized card member drops out of its card.
/// - A stage whose members all left the space takes the most recent card.
pub(crate) fn resolve(
    stage: &[HWND],
    order: &[Vec<HWND>],
    members: &[Member],
) -> (Vec<HWND>, Vec<Vec<HWND>>) {
    let top_level = |m: &&Member| m.root.is_none() && !m.sticky;
    let can_stage = |h: HWND| members.iter().filter(top_level).any(|m| m.hwnd == h);
    let can_card = |h: HWND| {
        members
            .iter()
            .filter(top_level)
            .any(|m| m.hwnd == h && !m.minimized)
    };

    let mut stage: Vec<HWND> = stage.iter().copied().filter(|&h| can_stage(h)).collect();
    let mut cards: Vec<Vec<HWND>> = order
        .iter()
        .map(|set| {
            set.iter()
                .copied()
                .filter(|&h| can_card(h) && !stage.contains(&h))
                .collect::<Vec<_>>()
        })
        .filter(|set| !set.is_empty())
        .collect();
    let newcomers: Vec<HWND> = members
        .iter()
        .filter(top_level)
        .filter(|m| {
            !m.minimized && !stage.contains(&m.hwnd) && !order.iter().any(|s| s.contains(&m.hwnd))
        })
        .map(|m| m.hwnd)
        .collect();
    let member = |h: HWND| members.iter().find(|m| m.hwnd == h);
    for h in newcomers {
        let joins = member(h).is_some_and(|m| {
            !m.framed
                && stage
                    .iter()
                    .any(|&s| member(s).is_some_and(|o| o.pid == m.pid))
        });
        if joins {
            stage.insert(0, h);
            continue;
        }
        let old: Vec<HWND> = stage.iter().copied().filter(|&o| can_card(o)).collect();
        if !old.is_empty() {
            cards.insert(0, old);
        }
        stage = vec![h];
    }
    if stage.is_empty() && !cards.is_empty() {
        stage = cards.remove(0);
    }
    (stage, cards)
}

/// Whether `hwnd` can come off the stage: another member stays showing, or a
/// card can take its place.
pub(crate) fn can_leave(
    stage: &[HWND],
    cards: &[Vec<HWND>],
    hwnd: HWND,
    showing: impl Fn(HWND) -> bool,
) -> bool {
    stage.contains(&hwnd) && (!cards.is_empty() || stage.iter().any(|&h| h != hwnd && showing(h)))
}

/// Take `hwnd` out of the stage set into a card of its own, first on the
/// strip. Refused when no other member of the set is showing: the stage would
/// be left empty and take that card straight back.
pub(crate) fn leave(
    stage: &mut Vec<HWND>,
    cards: &mut Vec<Vec<HWND>>,
    hwnd: HWND,
    showing: impl Fn(HWND) -> bool,
) -> bool {
    if !stage.contains(&hwnd) || !stage.iter().any(|&h| h != hwnd && showing(h)) {
        return false;
    }
    stage.retain(|&h| h != hwnd);
    cards.insert(0, vec![hwnd]);
    true
}

/// Whether `hwnd`, or the app window it belongs to, is in one of `cards`.
pub(crate) fn shelved_by(cards: &[Vec<HWND>], hwnd: HWND) -> bool {
    let in_cards = |h: HWND| cards.iter().any(|set| set.contains(&h));
    in_cards(hwnd) || app_root(hwnd).is_some_and(in_cards)
}

/// `hwnds` in z-order, topmost first; windows not found keep their order at
/// the end (hidden windows of a background space have no meaningful z-order).
fn z_sorted(hwnds: &[HWND]) -> Vec<HWND> {
    let mut sorted = Vec::with_capacity(hwnds.len());
    unsafe {
        let mut h = GetTopWindow(null_mut());
        while !h.is_null() && sorted.len() < hwnds.len() {
            if hwnds.contains(&h) {
                sorted.push(h);
            }
            h = GetWindow(h, GW_HWNDNEXT);
        }
    }
    let rest: Vec<HWND> = hwnds
        .iter()
        .copied()
        .filter(|h| !sorted.contains(h))
        .collect();
    sorted.extend(rest);
    sorted
}

/// Minimized by the user. A window we hid with the forced-minimize backend is
/// iconic too, so a hidden window answers from the flag the hide recorded.
fn window_pid(hwnd: HWND) -> u32 {
    let mut pid = 0;
    unsafe { GetWindowThreadProcessId(hwnd, &mut pid) };
    pid
}

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

/// Whether a card dragged to `pt` is dropped on the stage rather than back
/// on the strip: inside the stage area and clear of the strip by half its
/// thickness, so a reorder that strays a little stays a reorder.
pub(crate) fn over_stage(strip: &WindowRect, stage: &WindowRect, pt: POINT) -> bool {
    let margin = strip.width().min(strip.height()) / 2;
    let near = WindowRect {
        left: strip.left - margin,
        top: strip.top - margin,
        right: strip.right + margin,
        bottom: strip.bottom + margin,
    };
    stage.contains(pt) && !near.contains(pt)
}

/// `frame` moved so its centre sits on `at`.
fn centred_on(frame: &WindowRect, at: POINT) -> WindowRect {
    let (dx, dy) = (
        at.x - (frame.left + frame.right) / 2,
        at.y - (frame.top + frame.bottom) / 2,
    );
    WindowRect {
        left: frame.left + dx,
        top: frame.top + dy,
        right: frame.right + dx,
        bottom: frame.bottom + dy,
    }
}

/// The visible frame of a window that has one to measure: not minimized, which
/// a window hidden by the forced-minimize backend is.
fn frame_of(hwnd: HWND) -> Option<WindowRect> {
    if unsafe { IsIconic(hwnd) } != 0 {
        return None;
    }
    unsafe { winspaces_win32::dwm::extended_frame_bounds(hwnd) }.map(WindowRect::from)
}

/// Place `hwnd` so its visible frame is `target`.
fn move_frame(hwnd: HWND, target: &WindowRect) {
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
                root: app_root(hwnd),
                minimized: user_minimized(hwnd),
                sticky: self.is_sticky(hwnd),
                pid: window_pid(hwnd),
                framed: is_framed_window(hwnd),
            })
            .collect()
    }

    /// The stage and cards of a space, resolved against its members right
    /// now, with the stage in its current z-order.
    fn shelf_roles(&self, mon_idx: usize, space_idx: usize) -> (Vec<HWND>, Vec<Vec<HWND>>) {
        let entry = &self.monitors[mon_idx].shelf[space_idx];
        let stage = if entry.stage.len() > 1 {
            z_sorted(&entry.stage)
        } else {
            entry.stage.clone()
        };
        resolve(
            &stage,
            &entry.order,
            &self.shelf_members(mon_idx, space_idx),
        )
    }

    fn commit_roles(
        &mut self,
        mon_idx: usize,
        space_idx: usize,
        stage: Vec<HWND>,
        cards: Vec<Vec<HWND>>,
    ) {
        let entry = &mut self.monitors[mon_idx].shelf[space_idx];
        entry.stage = stage;
        entry.order = cards;
    }

    /// The committed cards of a space; none while Shelf is off.
    pub(crate) fn shelf_cards(&self, mon_idx: usize, space_idx: usize) -> &[Vec<HWND>] {
        if self.shelf_on(mon_idx, space_idx) {
            &self.monitors[mon_idx].shelf[space_idx].order
        } else {
            &[]
        }
    }

    pub(crate) fn is_shelved(&self, mon_idx: usize, space_idx: usize, hwnd: HWND) -> bool {
        shelved_by(self.shelf_cards(mon_idx, space_idx), hwnd)
    }

    /// The card set holding `hwnd`, or `hwnd` alone when it is not a card.
    pub fn shelf_set_of(&self, hwnd: HWND) -> Vec<HWND> {
        self.find_window(hwnd)
            .and_then(|(m, s)| {
                self.shelf_cards(m, s)
                    .iter()
                    .find(|set| set.contains(&hwnd))
                    .cloned()
            })
            .unwrap_or_else(|| vec![hwnd])
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

    /// The committed stage of a monitor's current space; empty while Shelf is
    /// off.
    pub fn shelf_stage_of(&self, mon_idx: usize) -> &[HWND] {
        let mon = &self.monitors[mon_idx];
        if self.shelf_on(mon_idx, mon.current) {
            &mon.shelf[mon.current].stage
        } else {
            &[]
        }
    }

    /// What a display's strip shows: its rect and the committed cards, each a
    /// set of windows with whether a live thumbnail is possible (a cloaked
    /// window keeps its surface; a minimized or `SW_HIDE`-hidden one does
    /// not). `None` when the current space has Shelf off or no cards.
    pub fn shelf_strip(&self, mon_idx: usize) -> Option<(WindowRect, StripCards)> {
        let (strip, _) = self.shelf_split(mon_idx)?;
        let mon = &self.monitors[mon_idx];
        let order = &mon.shelf[mon.current].order;
        if order.is_empty() {
            return None;
        }
        let live = WINSPACES_STATE_CLOAKED | WINSPACES_STATE_SHELL_CLOAKED;
        let cards = order
            .iter()
            .map(|set| {
                set.iter()
                    .map(|&h| (h, get_window_state(h) & live != 0))
                    .collect()
            })
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

    pub(crate) fn set_shelf_on(&mut self, mon_idx: usize, space_idx: usize, on: bool) {
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
                for stage in self.monitors[m].shelf[s].stage.clone() {
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
    /// cards of their own in z-order. Z-order rather than the foreground,
    /// because the toggle can come from Overview, which holds the foreground
    /// itself. Seeding `order` with every window is what keeps `resolve` from
    /// reading them as newcomers.
    fn seed_shelf(&mut self, mon_idx: usize, space_idx: usize) {
        let candidates: Vec<HWND> = self
            .shelf_members(mon_idx, space_idx)
            .into_iter()
            .filter(|m| m.root.is_none() && !m.sticky && !m.minimized)
            .map(|m| m.hwnd)
            .collect();
        let mut order = z_sorted(&candidates).into_iter();
        let stage: Vec<HWND> = order.next().into_iter().collect();
        let cards = order.map(|h| vec![h]).collect();
        self.commit_roles(mon_idx, space_idx, stage, cards);
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
    /// a newcomer takes the stage, a vacated stage takes the next card.
    pub fn shelf_apply(&mut self) {
        for m in 0..self.monitors.len() {
            let s = self.monitors[m].current;
            if !self.shelf_on(m, s) {
                continue;
            }
            let (stage, cards) = self.shelf_roles(m, s);
            let before = &self.monitors[m].shelf[s].stage;
            let entered: Vec<HWND> = stage
                .iter()
                .copied()
                .filter(|h| !before.contains(h))
                .collect();
            let changed = !entered.is_empty() || stage.len() != before.len();
            self.commit_roles(m, s, stage.clone(), cards);
            self.sync_shelf_visibility(m, s);
            if changed {
                for &h in &entered {
                    self.fit_stage(m, h);
                }
                unsafe {
                    let fg = foreground_root();
                    if let Some(&top) = stage.first() {
                        let on_stage = |h: HWND| stage.contains(&h);
                        if !on_stage(fg)
                            && !app_root(fg).is_some_and(on_stage)
                            && IsIconic(top) == 0
                        {
                            SetForegroundWindow(top);
                        }
                    }
                }
                self.mark_tiling_dirty(m, s);
            }
        }
    }

    /// Put the card set holding `hwnd` on stage, `hwnd` on top, in place of
    /// the current stage set, which becomes the most recent card. Returns
    /// whether the roles changed; nothing is shown or hidden.
    pub(crate) fn shelf_stage(&mut self, mon_idx: usize, space_idx: usize, hwnd: HWND) -> bool {
        if !self.shelf_on(mon_idx, space_idx) {
            return false;
        }
        let (stage, mut cards) = self.shelf_roles(mon_idx, space_idx);
        let Some(i) = cards.iter().position(|set| set.contains(&hwnd)) else {
            return false;
        };
        let mut set = cards.remove(i);
        set.retain(|&h| h != hwnd);
        set.insert(0, hwnd);
        let old: Vec<HWND> = stage.into_iter().filter(|&h| !user_minimized(h)).collect();
        if !old.is_empty() {
            cards.insert(0, old);
        }
        self.commit_roles(mon_idx, space_idx, set, cards);
        true
    }

    /// A card was clicked or one of its windows activated: swap its set onto
    /// the stage. The new stage is shown and focused before the old one is
    /// hidden, so the OS never has to pick a window to activate in between.
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
            let set = self.monitors[m].shelf[s].stage.clone();
            self.bring_on_stage(m, s, &set, hwnd, &[]);
        }
        self.mark_tiling_dirty(m, s);
    }

    /// The card holding `hwnd` was dropped on the stage (`at`, screen
    /// coordinates) or Ctrl+clicked (`None`): its set joins the windows on
    /// stage instead of replacing them.
    #[allow(clippy::not_unsafe_ptr_arg_deref)]
    pub fn shelf_join(&mut self, hwnd: HWND, at: Option<POINT>) {
        let Some((m, s)) = self.find_window(hwnd) else {
            return;
        };
        if self.monitors[m].current != s || !self.shelf_on(m, s) {
            return;
        }
        let (mut stage, mut cards) = self.shelf_roles(m, s);
        let Some(i) = cards.iter().position(|set| set.contains(&hwnd)) else {
            return;
        };
        let mut set = cards.remove(i);
        set.retain(|&h| h != hwnd);
        set.insert(0, hwnd);
        let placed = at.map_or_else(Vec::new, |pt| self.floating_drop(m, &set, pt));
        stage.splice(0..0, set.iter().copied());
        self.commit_roles(m, s, stage, cards);
        log_info!(
            "Shelf: {} window(s) of hwnd {:?} join the stage on Mon {}, Space {}",
            set.len(),
            hwnd,
            m + 1,
            s + 1
        );
        self.bring_on_stage(m, s, &set, hwnd, &placed);
        self.mark_tiling_dirty(m, s);
    }

    /// Show `set`, place it (`placed` where given, else out from under the
    /// strip), focus `focus`, then hide whatever no longer belongs on screen.
    fn bring_on_stage(
        &mut self,
        mon_idx: usize,
        space_idx: usize,
        set: &[HWND],
        focus: HWND,
        placed: &[(HWND, WindowRect)],
    ) {
        for &h in set {
            set_window_visibility(h, true, self.show_all_taskbar);
            match placed.iter().find(|(p, _)| *p == h) {
                Some((_, target)) => move_frame(h, target),
                None => self.fit_stage(mon_idx, h),
            }
        }
        unsafe {
            SetForegroundWindow(focus);
        }
        self.sync_shelf_visibility(mon_idx, space_idx);
    }

    /// Whether the tiler places `hwnd` rather than the user.
    fn tiler_places(&self, hwnd: HWND) -> bool {
        self.tiling_enabled && !self.is_floating(hwnd) && is_tileable_window(hwnd)
    }

    /// Where the floating windows of `set` land when it is dropped at `at`:
    /// moved together so the first one's centre is on `at`, each clamped
    /// into the stage area.
    fn floating_drop(&self, mon_idx: usize, set: &[HWND], at: POINT) -> Vec<(HWND, WindowRect)> {
        let area = self.stage_area(mon_idx);
        let Some(lead) = set.first().and_then(|&h| frame_of(h)) else {
            return Vec::new();
        };
        let moved = centred_on(&lead, at);
        let (dx, dy) = (moved.left - lead.left, moved.top - lead.top);
        set.iter()
            .filter(|&&h| !self.tiler_places(h))
            .filter_map(|&h| {
                let f = frame_of(h)?;
                let shifted = WindowRect {
                    left: f.left + dx,
                    top: f.top + dy,
                    right: f.right + dx,
                    bottom: f.bottom + dy,
                };
                Some((h, clamp_to_work(&shifted, &area)))
            })
            .collect()
    }

    /// While the card holding `hwnd` is dragged to `at`: `None` when a drop
    /// there goes back to the strip, else the stage area and where the
    /// card's windows would land (the tile layout the drop would produce,
    /// when the tiler places them).
    pub fn shelf_join_preview(
        &self,
        hwnd: HWND,
        at: POINT,
    ) -> Option<(WindowRect, Vec<WindowRect>)> {
        let (m, s) = self.find_window(hwnd)?;
        if self.monitors[m].current != s {
            return None;
        }
        let (strip, area) = self.shelf_split(m)?;
        if !over_stage(&strip, &area, at) {
            return None;
        }
        let set = self
            .shelf_cards(m, s)
            .iter()
            .find(|set| set.contains(&hwnd))?
            .clone();
        let tiled = set.iter().filter(|&&h| self.tiler_places(h)).count();
        let mut rects: Vec<WindowRect> = self
            .floating_drop(m, &set, at)
            .into_iter()
            .map(|(_, r)| r)
            .collect();
        if tiled > 0 {
            rects.extend(self.tile_layout_with(m, s, tiled));
        }
        Some((area, rects))
    }

    /// Where `hwnd` sits and the strip it can be dropped on to come off the
    /// stage, if it can.
    fn leave_strip(&self, hwnd: HWND) -> Option<(usize, usize, WindowRect)> {
        let (m, s) = self.find_window(hwnd)?;
        if self.monitors[m].current != s {
            return None;
        }
        let (strip, _) = self.shelf_split(m)?;
        let entry = &self.monitors[m].shelf[s];
        can_leave(&entry.stage, &entry.order, hwnd, |h| !user_minimized(h)).then_some((m, s, strip))
    }

    pub fn shelf_can_leave(&self, hwnd: HWND) -> bool {
        self.leave_strip(hwnd).is_some()
    }

    /// While the stage window `hwnd` is moved with the cursor at `at`: the
    /// strip to highlight as its drop target. Only under the cursor, unless
    /// the strip has no cards and so nothing on screen to aim for.
    pub fn shelf_leave_zone(&self, hwnd: HWND, at: POINT) -> Option<WindowRect> {
        let (m, s, strip) = self.leave_strip(hwnd)?;
        (strip.contains(at) || self.shelf_cards(m, s).is_empty()).then_some(strip)
    }

    /// A stage window was dragged and dropped at `at` (screen coordinates):
    /// over the strip, it leaves its set and becomes the first card; alone on
    /// stage, the most recent card takes its place. Returns whether it left.
    #[allow(clippy::not_unsafe_ptr_arg_deref)]
    pub fn shelf_leave(&mut self, hwnd: HWND, at: POINT) -> bool {
        let Some((m, s, strip)) = self.leave_strip(hwnd) else {
            return false;
        };
        if !strip.contains(at) {
            return false;
        }
        let (mut stage, mut cards) = self.shelf_roles(m, s);
        if !leave(&mut stage, &mut cards, hwnd, |h| !user_minimized(h)) {
            let next = cards.first().and_then(|set| set.first().copied());
            let Some(next) = next.filter(|_| stage.contains(&hwnd)) else {
                return false;
            };
            self.tiling_drag = None;
            self.shelf_promote(next);
            return true;
        }
        let top = stage.iter().copied().find(|&h| !user_minimized(h));
        self.commit_roles(m, s, stage, cards);
        // The window is hidden: its drop is not the tiler's to classify.
        self.tiling_drag = None;
        log_info!(
            "Shelf: hwnd {:?} leaves the stage on Mon {}, Space {}",
            hwnd,
            m + 1,
            s + 1
        );
        if let Some(top) = top {
            unsafe {
                SetForegroundWindow(top);
            }
        }
        self.sync_shelf_visibility(m, s);
        self.mark_tiling_dirty(m, s);
        true
    }

    /// Split the card set holding `hwnd` into one card per window, in place.
    pub fn shelf_separate(&mut self, hwnd: HWND) {
        let Some((m, s)) = self.find_window(hwnd) else {
            return;
        };
        let order = &mut self.monitors[m].shelf[s].order;
        if let Some(i) = order.iter().position(|set| set.contains(&hwnd)) {
            let set = order.remove(i);
            for (j, h) in set.into_iter().enumerate() {
                order.insert(i + j, vec![h]);
            }
        }
        request_shelf_refresh();
    }

    /// Reorder the cards of a monitor's current space. `order` names each card
    /// by any of its windows; cards missing from it (past the end of the
    /// strip) keep their relative order after it.
    pub fn shelf_reorder(&mut self, mon_idx: usize, order: &[HWND]) {
        let Some(mon) = self.monitors.get_mut(mon_idx) else {
            return;
        };
        let entry = &mut mon.shelf[mon.current];
        let mut next: Vec<Vec<HWND>> = Vec::with_capacity(entry.order.len());
        for h in order {
            if let Some(set) = entry.order.iter().find(|set| set.contains(h)) {
                if !next.contains(set) {
                    next.push(set.clone());
                }
            }
        }
        for set in &entry.order {
            if !next.contains(set) {
                next.push(set.clone());
            }
        }
        entry.order = next;
        request_shelf_refresh();
    }

    /// Move a window entering the stage out from under the strip. Only on
    /// entry: the user can still drag it over the strip afterwards. Maximized
    /// windows are left alone (maximize is fullscreen) and tiled ones belong
    /// to the tiler.
    fn fit_stage(&self, mon_idx: usize, hwnd: HWND) {
        unsafe {
            if IsZoomed(hwnd) != 0 {
                return;
            }
        }
        if self.tiler_places(hwnd) {
            return;
        }
        let Some(frame) = frame_of(hwnd) else {
            return;
        };
        let target = clamp_to_work(&frame, &self.stage_area(mon_idx));
        if target != frame {
            move_frame(hwnd, &target);
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
            pid: hwnd as u32,
            framed: true,
        }
    }

    fn minimized(hwnd: HWND) -> Member {
        Member {
            minimized: true,
            ..top(hwnd)
        }
    }

    #[test]
    fn settled_roles_are_kept() {
        let members = [top(A), top(B), top(C)];
        assert_eq!(
            resolve(&[A], &[vec![B], vec![C]], &members),
            (vec![A], vec![vec![B], vec![C]])
        );
    }

    #[test]
    fn a_newcomer_takes_the_stage() {
        let members = [top(A), top(B), top(C)];
        assert_eq!(
            resolve(&[A], &[vec![B]], &members),
            (vec![C], vec![vec![A], vec![B]])
        );
    }

    #[test]
    fn a_newcomer_sends_the_whole_stage_set_to_one_card() {
        let members = [top(A), top(B), top(C), top(D)];
        assert_eq!(
            resolve(&[A, B], &[vec![C]], &members),
            (vec![D], vec![vec![A, B], vec![C]])
        );
    }

    #[test]
    fn a_frameless_newcomer_joins_its_app_on_stage() {
        let viewer = |pid| Member {
            pid,
            framed: false,
            ..top(D)
        };
        let members = [top(A), top(B), viewer(1)];
        assert_eq!(
            resolve(&[A], &[vec![B]], &members),
            (vec![D, A], vec![vec![B]])
        );
        let members = [top(A), top(B), viewer(2)];
        assert_eq!(
            resolve(&[A], &[vec![B]], &members),
            (vec![D], vec![vec![A], vec![B]])
        );
    }

    #[test]
    fn a_stage_window_leaves_into_the_first_card() {
        let (mut stage, mut cards) = (vec![A, B], vec![vec![C]]);
        assert!(leave(&mut stage, &mut cards, B, |_| true));
        assert_eq!((stage, cards), (vec![A], vec![vec![B], vec![C]]));
    }

    #[test]
    fn the_last_showing_stage_window_cannot_leave() {
        let (mut stage, mut cards) = (vec![A], vec![vec![C]]);
        assert!(!leave(&mut stage, &mut cards, A, |_| true));
        let mut stage = vec![A, B];
        assert!(!leave(&mut stage, &mut cards, A, |h| h != B));
        assert!(!leave(&mut stage, &mut cards, C, |_| true));
        assert_eq!((stage, cards), (vec![A, B], vec![vec![C]]));
    }

    #[test]
    fn a_stage_window_can_leave_while_something_can_replace_it() {
        let showing = |_| true;
        assert!(can_leave(&[A, B], &[], A, showing), "the set stays");
        assert!(can_leave(&[A], &[vec![B]], A, showing), "a card swaps in");
        assert!(!can_leave(&[A], &[], A, showing), "nothing to show");
        assert!(!can_leave(&[A, B], &[], A, |h| h != B), "B is minimized");
        assert!(!can_leave(&[A], &[vec![B]], B, showing), "not on stage");
    }

    #[test]
    fn a_vacated_stage_goes_to_the_most_recent_card() {
        let members = [top(B), top(C), top(D)];
        assert_eq!(
            resolve(&[A], &[vec![C, D], vec![B]], &members),
            (vec![C, D], vec![vec![B]])
        );
    }

    #[test]
    fn a_set_member_closing_leaves_the_rest_on_stage() {
        let members = [top(B), top(C)];
        assert_eq!(
            resolve(&[A, B], &[vec![C]], &members),
            (vec![B], vec![vec![C]])
        );
    }

    #[test]
    fn a_minimized_stage_stays_and_pulls_nothing_up() {
        let members = [minimized(A), top(B)];
        assert_eq!(
            resolve(&[A], &[vec![B]], &members),
            (vec![A], vec![vec![B]])
        );
    }

    #[test]
    fn a_minimized_set_member_stays_in_the_stage_set() {
        let members = [top(A), minimized(B), top(C)];
        assert_eq!(
            resolve(&[A, B], &[vec![C]], &members),
            (vec![A, B], vec![vec![C]])
        );
    }

    #[test]
    fn a_newcomer_drops_minimized_stage_members_from_the_card() {
        let members = [minimized(A), top(B), top(C)];
        assert_eq!(
            resolve(&[A], &[vec![B]], &members),
            (vec![C], vec![vec![B]])
        );
        let members = [top(A), minimized(B), top(C), top(D)];
        assert_eq!(
            resolve(&[A, B], &[vec![C]], &members),
            (vec![D], vec![vec![A], vec![C]])
        );
    }

    #[test]
    fn a_minimized_card_member_drops_out_of_its_card() {
        let members = [top(A), top(B), minimized(C)];
        assert_eq!(
            resolve(&[A], &[vec![B, C]], &members),
            (vec![A], vec![vec![B]])
        );
    }

    #[test]
    fn owned_pinned_and_minimized_windows_are_never_cards() {
        let mut b = top(B);
        b.root = Some(A);
        let mut c = top(C);
        c.sticky = true;
        let members = [top(A), b, c, minimized(D)];
        assert_eq!(resolve(&[A], &[], &members), (vec![A], vec![]));
    }

    #[test]
    fn stale_cards_are_dropped() {
        let members = [top(A), top(C)];
        assert_eq!(
            resolve(&[A], &[vec![B], vec![C]], &members),
            (vec![A], vec![vec![C]])
        );
    }

    #[test]
    fn an_empty_stage_is_filled_from_the_cards() {
        let members = [top(A), top(B)];
        assert_eq!(
            resolve(&[], &[vec![B], vec![A]], &members),
            (vec![B], vec![vec![A]])
        );
    }

    #[test]
    fn shelved_by_looks_inside_every_set() {
        assert!(shelved_by(&[vec![A, B]], B));
        assert!(!shelved_by(&[vec![A, B]], C));
    }

    #[test]
    fn a_drop_joins_only_clear_of_the_strip() {
        let work = WindowRect {
            left: 0,
            top: 0,
            right: 1920,
            bottom: 1040,
        };
        let (strip, stage) = split_work(&work, ShelfSide::Left, 96);
        let at = |x| POINT { x, y: 500 };
        assert!(!over_stage(&strip, &stage, at(150)), "on the strip");
        assert!(!over_stage(&strip, &stage, at(250)), "straying off it");
        assert!(over_stage(&strip, &stage, at(400)));
        assert!(!over_stage(&strip, &stage, at(2500)), "another display");
    }

    #[test]
    fn a_dropped_frame_is_centred_on_the_cursor() {
        let f = WindowRect {
            left: 0,
            top: 0,
            right: 400,
            bottom: 300,
        };
        let c = centred_on(&f, POINT { x: 1000, y: 500 });
        assert_eq!((c.left, c.top, c.width(), c.height()), (800, 350, 400, 300));
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
