# Tiling Shakedown

One pass over everything the tiling feature shipped (phases 1-7 plus the
live-testing fix chain), on the live daemon. Run the sections in order — later
steps assume the earlier ones held. Tick items by editing this file.

## 0 · Setup

Fresh start so behavior isn't inherited from a stale session.

- [ ] Build and relaunch the daemon from `feature/window-tile-manager`; keep a log tail open the whole run.
- [ ] Startup log shows every tiling hotkey registering. → no `[FAIL]` lines; tiling starts in the state saved in config.

## 1 · Auto-tiling basics

One monitor, one space, classic apps.

- [ ] One Explorer window open, press `Ctrl+Alt+Shift+T`. → window fills the work area (taskbar untouched), corners square.
- [ ] Open Notepad. → 50/50 side-by-side within ~1s, edges flush, zero overlap.
- [ ] Open a third window. → right half splits top/bottom (dwindle spiral).
- [ ] Close the middle window. → survivors re-expand within ~1s.
- [ ] Press `Ctrl+Alt+Shift+T` again, then drag a window around. → windows stay put, corners rounded again, nothing fights the drag.

## 2 · Awkward windows

- [ ] Tiling on, add Calculator (UWP) to the mix. → tiles flush like the classic apps — no 8px gap from invisible borders.
- [ ] Add Fork (borderless custom chrome, zero DWM shadow margins). → tiles edge-exact, no verify mismatch or auto-float in the log.
- [ ] Open `Win+R`. → the Run dialog floats, never resized.
- [ ] Minimize one tile, then restore it. → others expand while it's gone; it returns to its old slot.
- [ ] Maximize one tile. → flattened back into its slot on the next re-layout; to keep a window maximized over the tiles, float it first (`Ctrl+Alt+Shift+F`), then maximize.
- [ ] Restore a workspace layout containing a maximized window onto a tiled space, then open a second window. → the restored window is un-maximized and both tile 50/50; no maximize/restore fight in the log.
- [ ] Squeeze a Chromium window into a tiny tile (open 5-6 windows). → if it can't fit, it auto-floats after two verify strikes, with a log line.

## 3 · Keyboard control

Three windows tiled on one space.

- [ ] `Ctrl+Alt+Shift` + arrows. → focus moves to the correct neighbor in all four directions.
- [ ] `Ctrl+Shift+Win` + arrows. → focused window trades slots with its neighbor; focus follows the window.
- [ ] `Ctrl+Alt+Shift+-` / `+` repeatedly. → first split steps 5% each press, clamps at 10% / 90%.
- [ ] `Ctrl+Alt+Shift+F` on a tile, then again. → floats in place while others retile; unfloat re-inserts it beside the focused slot.
- [ ] Toggle tiling off, press each tiling hotkey once. → log line per press, no window moves.

## 4 · Gaps & mixed DPI

- [ ] Settings → Tiling: inner gap 16, outer gap 8. → gutters appear on the tiled space immediately, no restart.
- [ ] Compare the BenQ and the HP with tiled spaces on both. → gap physical size tracks each display's DPI, not raw pixels.
- [ ] Set both gaps back to 0. → tiles flush again.

## 5 · Mouse gestures

Start with exactly two tiled windows.

- [ ] Drag the shared edge. → split ratio follows the drag and sticks; opening a third window keeps the new ratio.
- [ ] Title-bar drag window A, drop the cursor over window B. → the two swap slots on release.
- [ ] Drag a tile and drop it over the taskbar / nowhere useful. → clean snap back to its slot.
- [ ] Drag a floated window around on the tiled space. → completely untouched — no snap, no retile.
- [ ] With three windows, border-drag a deep split edge. → snaps back — known v1 limit; keyboard ratio keys cover this case.

## 6 · Spaces, topology, restart

> The RDP step replaces the whole display topology — do it with work saved.

- [ ] Send a window to a hidden space (`Ctrl+Alt+2`), then follow it (`Alt+2`). → correctly tiled on arrival, no visible reflow beyond one frame.
- [ ] Switch away and back (`Alt+2`, `Alt+1`). → layout identical, ratios kept.
- [ ] RDP in from the laptop via mstsc, then disconnect back at the desk (2→1→2). → after the ~1.2s reconcile, tiled spaces re-fit both monitors; floating windows back at their old rects; check actual rects, not just log lines.
- [ ] `winspaces --exit`, relaunch. → tiling re-enables from config and spaces re-tile; ratios reset to 50% — documented behavior.

## 7 · Settings UI

- [ ] Open the Tiling page. → enable toggle, both gap combos, and all 12 hotkey recorders render; `+`/`-` show as symbols, not VK codes.
- [ ] Rebind the toggle to `Ctrl+Alt+Shift+G`. → old binding dead, new one live, no restart.
- [ ] Record a conflicting hotkey (try `Alt+1`). → the existing rollback-and-reopen-settings behavior — never silent loss of all hotkeys.
- [ ] Close and reopen settings. → every value persisted.

## 8 · Float rules, CLI, Mission Control

- [ ] Hand-add a float rule for Calculator under `tiling.float_rules` in settings.json, reload. → Calculator floats on tiled spaces, and still does after a daemon restart.
- [ ] Run `winspaces --tiling-toggle` (or `-t`) from a terminal. → tiling toggles; if the daemon runs elevated, it still works from a normal shell.
- [ ] Open Mission Control with tiling on. → space cards show the *Tiled* tag; dragging a window card onto a space tiles it there on switch.

## 9 · Log review

Skim the whole tail from the run before signing off.

- [ ] No retile/enforce ping-pong — alternating reposition lines repeating for the same window.
- [ ] No `AppState busy (re-entrant event)` spam correlated with tiling actions.
- [ ] Every auto-float has a matching strike/resist or refused-to-un-maximize log line explaining it.
- [ ] No repeating `flatten pending` lines — a still-maximized window must resolve or auto-float within 3 attempts.

---

Anything misbehaves: note the section number and grab the log tail from that
moment — that pair is enough to triage.
