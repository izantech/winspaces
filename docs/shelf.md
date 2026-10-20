# Shelf

Shelf is a per-space mode for switching quickly between a handful of windows.
One window is **on stage**: visible, focused, and laid out in the monitor's
work area minus a strip along one edge. Every other window of the space is
hidden with the usual cloak and shown in that strip as a **card** with a live
thumbnail. Clicking a card swaps it onto the stage, and the window that was on
stage becomes the most recent card. macOS users will recognise the idea; it is
built here on the same hide/show machinery as spaces themselves.

*Last verified: 2026-10-20, against e0b9760.*

---

## 1. Turning It On

- The toggle hotkey (`Ctrl+Alt+Shift+H` by default) flips Shelf on the focused
  display's current space. Per-space state is session state, like tiling's
  per-space layout: a restart starts with every space off.
- With the **all spaces** setting (`shelf.all_spaces`, on the Tiling page of
  Settings next to the strip's edge and the shortcut), the same toggle flips
  Shelf on every space of every display at once and the result persists
  (`shelf.enabled`). `SpaceManager::shelf_everywhere` holds it; turning it on
  or off clears the per-space toggles, so off means off everywhere.
- Turning Shelf on seeds the roles from the z-order: the topmost window of
  the space takes the stage and the rest become cards, top first. Not the
  foreground window, because the toggle can come from Overview's button, and
  Overview holds the foreground itself.
  Turning it off forgets the roles and shows every card again.

## 2. Roles Are Derived, Not Stored Twice

`MonitorState::shelf` is a `Vec<ShelfSpace>` parallel to `spaces` (kept at
the same six touch points as `tiling`), holding the last committed `stage` and
card `order`. Nothing records which windows Shelf hid. Every pass resolves the
roles from the committed state and the space's current members with the pure
`shelf::resolve`:

- Owned windows (dialogs, palettes) and pinned windows are never on stage nor
  cards. An owned window follows its root: hidden while the root is a card.
- A member that is neither on stage nor a card is **new to the space**: just
  opened, moved in (`Ctrl+Alt+N`, an Overview drop), restored from minimized,
  or unpinned. It takes the stage and the previous stage becomes the first
  card. This one rule is how a new window, Alt+Tab to a restored window and a
  drop from Overview all behave without a handler each.
- A stage that left the space (closed, moved, pinned) hands the stage to the
  most recent card.
- A minimized stage keeps the stage, which is then empty; nothing cascades.
  "Minimized" for a window we hid comes from `WAS_ICONIC`, because the
  forced-minimize backend leaves our own hidden windows iconic too.

`should_be_visible` (`spaces/sticky.rs`) consults `is_shelved`, so every
visibility pass (`switch_space`, `reapply_visibility`, `set_show_all_taskbar`)
agrees with Shelf without knowing it exists. Hiding goes through
`set_window_visibility`, so the hidden-state invariant of
[`dwm.md`](dwm.md) §5.3 holds unchanged, and exit and `dev recover` restore
cards like any other hidden window.

## 3. Swapping and Refreshing

- `shelf_promote` (a card clicked, or a card activated from the taskbar,
  which `resolve_activation` reports as `ActivationDecision::Promote`) shows
  the card, moves it out from under the strip, focuses it, and only then hides
  the old stage, so the OS never has to pick a window to activate in between.
  The activation echo resolves to `Ignore`: the window is already on stage.
- `switch_space` with a window to activate puts it on stage first when it is a
  card, so focusing a card from Overview lands on it.
- Everything else (windows opening, closing, minimizing, moving, pins) goes
  through one refresh. Core calls `schedule_shelf` from `mark_tiling_dirty`,
  `detach_window`, `prune_dead_windows`, `set_sticky` and the display-change
  path, only while some Shelf is on; the bin debounces it with
  `TIMER_SHELF` and runs `refresh_shelf`, which commits the roles
  (`shelf_apply`) and makes the screen match.

## 4. Placement

`split_work` cuts the monitor's work area into the strip and the stage area
(200 px wide on the sides, 150 px tall on the top or bottom, at 96 DPI). The
tiler lays out the stage area instead of the full work area, and cards are
never tile candidates, so a tiled Shelf space holds one tile. A floating window
entering the stage is clamped into the stage area once, on entry; the user can
still drag it over the strip afterwards. A maximized stage window is left
maximized: maximize is fullscreen, as in [`tiling.md`](tiling.md) §5.5.

## 5. The Strip

`winspaces-ui`'s `shelf` module owns one window per display, created on first
use and hidden while its display shows no cards. `refresh_shelf` hands it a
`StripModel` per display (the strip rect from `SpaceManager::shelf_strip` and
the committed cards, most recent first); the strip never reads daemon state.

- **Window.** `WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE`, answering
  `WM_MOUSEACTIVATE` with `MA_NOACTIVATE`, so clicking a card never takes the
  focus from the stage. Non-layered, with the acrylic backdrop and rounded
  corners, because a DWM thumbnail cannot be hosted in a layered window. It is
  inset a few pixels into the reserved rect so it reads as a panel. The
  daemon's own windows are never tracked (`is_valid_window` rejects the
  daemon's process), so the strip is never a member of a space. While
  Overview is open, a strip shown is inserted beneath it.
- **Cards.** A card whose window carries `CLOAKED` or `SHELL_CLOAKED` shows a
  live thumbnail, registered once and reused across refreshes like
  Overview's. A window hidden by the forced-minimize or `SW_HIDE` fallback
  has no surface to show, so its card draws the window's icon and title.
  Cards keep their natural size while they fit, shrink to fit, and past a
  minimum size the ones that still do not fit are left out of the strip;
  Overview and the taskbar still reach them.
- **Gestures.** A press arms the card; a release before the system drag
  threshold is a click, which calls `ShelfHost::promote`. Past the threshold
  along the strip's axis the card's thumbnail follows the pointer on that axis
  only, and the release drops it past every card whose centre it crossed
  (`ShelfHost::reorder`). Movement across the axis is ignored, never a cancel.
  Host calls run after the strip's state borrow is released, because the host
  re-enters `sync`.
- **Fullscreen stage.** The strip hides while the stage window fills its
  monitor (maximized or borderless fullscreen). The location hook checks each
  display's stage against the state recorded at the last refresh and asks for
  a refresh when it flips; every other window costs one comparison.

## 6. Alternatives Considered and Rejected

| Alternative | Why not |
|---|---|
| Registering the strip as a shell AppBar (`SHAppBarMessage`) so maximize respects it | It changes the system work area, so every toggle and every space switch would re-maximize every window on the monitor, hidden ones included, and `MonitorState::work` would drift under the rest of the daemon. |
| A set of shelved windows next to the space lists | A second source of truth that drifts from membership, the class of bug [`dwm.md`](dwm.md) §5.3 records. Derived roles cannot disagree with the space lists. |
| Shrinking the strip windows instead of hiding them | An out-of-process manager cannot scale another process's window; thumbnails of a cloaked window are live (checked against a playing video under the shell cloak), so hiding costs nothing visible. |
| A daemon-side monocle mode for the stage | Maximize is the fullscreen mode; see [`tiling.md`](tiling.md) §5.5. |

## See also

- [`dwm.md`](dwm.md) — the cloaking backends and the hidden-state invariant.
- [`tiling.md`](tiling.md) — the layout the stage area feeds.
- [`overview.md`](overview.md) — Overview still lists every window of a
  space, cards included.
- [`ipc-and-config.md`](ipc-and-config.md) §4 — the `shelf` settings block.
