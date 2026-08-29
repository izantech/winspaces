# WhatsApp Web fills the HP monitor while Telegram shares the space

Investigated 2026-08-29 against the live daemon (log at
`%LOCALAPPDATA%\winspaces\winspaces.log`) and read-only Win32 queries on the
three windows tracked in Mon 2 / Space 2. Nothing was moved, focused or
restarted.

## Symptom

On the HP monitor (`\\.\DISPLAY1`, 1920x1200 physical, 125 % DPI, secondary),
Space 2 holds WhatsApp Web and Telegram. WhatsApp always takes the whole work
area; Telegram sits on top of it on the right half.

## Live window facts

| hwnd | window | class | rect (logical) | notes |
|---|---|---|---|---|
| `0xe40922` | WhatsApp Web (Brave PWA, `BraveOrigin._crx_hnpfjng…`) | `Chrome_WidgetWin_1` | (-1542,534)-(7,1452) = **full work area** | not zoomed, not cloaked, `showCmd=1` |
| `0x3b0646` | Telegram main window | `Qt51519QWindowIcon` | (-774,534)-(7,1452) = right half | not zoomed, visible |
| `0x10d055a` | Telegram "Media viewer" | `Qt51519QWindowIcon` | 1536x960 | `WS_POPUP`, invisible; matches the Telegram rule by exe and gets tracked/auto-placed |

Every `flush_retile: Mon 2 Space 2` line since 22:22 reports
**`retiled 1 window(s)`**: the tiler sees a single tile on that space. That
tile is WhatsApp, so it legitimately receives 100 % of the work area. The
window is not maximized and nothing is pushing it there; the layout is correct
for one tile.

## Root cause: Telegram was auto-floated on 2026-08-28 and never came back

`winspaces.log` lines 11455-11480, topology collapsed to the HP monitor alone
(`HWP2956` only, RDP session). Mon 1 / Space 2 had five tiles, so Telegram got a
quarter tile of 960x570 physical pixels:

```
verify_retile: hwnd 0x3b0646 mismatch (actual=… bottom: 583, expected=… bottom: 570), strike 1/4
… strike 2/4, 3/4 …
verify_retile: hwnd 0x3b0646 resisted tiling for 4 passes; auto-floating and restoring corner rounding
```

Telegram enforces a minimum window height (`WM_GETMINMAXINFO`); it accepted the
position but refused the last 13 px. 13 px is above the 8 px verify tolerance,
so after four sweeps (`verify_retile`, `crates/winspaces-core/src/tiling/engine.rs:341-372`)
the window was inserted into `SpaceManager.floating_windows`. Two other windows
(`0x1e0726`, `0x2107b0`, min-width 627 vs 480 requested) were floated in the
same sweep.

Since `333850d refactor(tiling): make floating state window-scoped across
spaces`, that set is keyed by hwnd and survives every space switch, monitor
move and topology change. It is only cleared by `remove_window` (window
destroyed), `windows_show_all` (tray "show all" / session end) or an explicit
`tiling_toggle_float`. Telegram is a long-lived window, so the auto-float taken
under a transient five-tile crunch on one monitor followed it to a space where
it would have fit comfortably. Today Mon 2 / Space 2 = {WhatsApp tiled,
Telegram floating}, hence one full-screen tile with a floater on top.

The `layouts.json` on disk has no floating data, so this is purely in-memory
session state; a daemon restart would also clear it (but would swap the daemon
under the live session, see the console-detach notes).

## Immediate remedy (no code)

Focus Telegram and press the toggle-float hotkey (`Ctrl+Alt+Shift+F` by
default, `default_tiling_toggle_float_hotkey` in
`crates/winspaces-common/src/config.rs:187`). `tiling_toggle_float` removes the
hwnd from `floating_windows` and re-flushes; WhatsApp and Telegram become a
50/50 dwindle pair on the HP.

## Proposed fix

The auto-float is a *measurement made under one layout* being stored as a
*permanent property of the window*. Two changes, in order of value:

### 1. Separate auto-floats from user floats and let them heal

Add `SpaceManager.auto_floated: HashSet<HWND>` next to `floating_windows`.
`verify_retile`/`flush_retile` strike-outs insert into both; `is_floating`
keeps reading `floating_windows`, so nothing downstream changes.

Re-admit an auto-floated window (remove from both sets, log) when the
constraint that produced the strike-out is gone:

- the window is re-tracked to a different monitor or space
  (`track_window` / `move_window_to_space` paths that already touch
  `floating_windows` in `remove_window`), or
- the candidate count of its space **drops** (a tile got bigger), tested in
  `flush_retile` by comparing `ready_candidates.len()` with the previous
  `ts.order.len()`.

If it resists again it simply strikes out again (four sweeps, ~1 s). User
floats made with `tiling_toggle_float` are never in `auto_floated` and stay
sticky as today. `tiling_toggle_float` clears the hwnd from both sets so an
explicit un-float is authoritative.

Tests: strike-out marks both sets; re-track to another monitor clears only
auto-floats; toggle_float on an auto-floated window leaves it tiled and
removes it from `auto_floated`; the existing "toggling tiling off/on keeps
floating_windows" test gains the same assertion for `auto_floated`.

### 2. Do not strike a window that only overflows its tile

In `verify_retile`, when `dx <= 8 && dy <= 8` (position honoured) and the
actual size is **larger** than expected, the window is clamped by its own
minimum size, not fighting the tiler. Hyprland treats these the same way: the
tile is placed, the window overflows by its minimum. Log once at info level and
clear strikes instead of counting them; keep striking on position mismatches,
on windows that are *smaller* than their tile, or on windows that moved
themselves. This stops most min-size auto-floats (Telegram 13 px, the two
627-vs-480 windows) from ever happening while still catching elevated windows
under UIPI, which do not move at all.

With (1) alone the bug heals on the next re-track; with (2) it never occurs for
min-size windows. Both are small, local to `engine.rs` and `manager.rs`.

## Side finding

`0x10d055a` is Telegram's hidden media-viewer popup (`WS_POPUP`, no caption,
`IsWindowVisible == false`). It matches the `Telegram.exe` rule by exe name,
is tracked into Mon 2 / Space 2 and bounced across monitors on activation
(`Window 0x10d055a moved across displays from Mon 2 to Mon 1 … on activation`
followed by a re-placement). It is not a tiling candidate so it does not affect
the layout, but rule matching should probably require a visible, captioned
top-level window before auto-placing.

Fixed on 2026-08-29: `is_framed_window` (`eligibility.rs`, `WS_CAPTION` both
bits) gates `match_rule_for_window` — so the ShellHook, activation and
startup-restore paths all refuse to place a frameless popup — and
`capture_active_workspace_detailed`, so the `Media viewer` snapshot entry
stops being written. Tracking is unchanged: the viewer still hides with its
space while Telegram shows it.

## Status

Both parts implemented on 2026-08-29 (`SpaceManager.auto_floated`,
`readmit_auto_floated`, the candidate-count check in `flush_retile`, the
overflow branch and `TileSpace.overflowing` in `verify_retile`); documented in
`docs/tiling.md` under the verify sweep. The running daemon predates the change,
so Telegram's current float is still in its memory: un-float it with the hotkey
or restart the daemon once the new build is in place.
