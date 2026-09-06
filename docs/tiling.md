# Dynamic Tiling Window Manager

WinSpaces includes an optional Hyprland-inspired dynamic tiling engine. When enabled, managed windows on each space are arranged automatically in a BSP / spiral dwindle hierarchy with zero overlapping frames and configurable inner/outer gaps.

*Last verified: 2026-09-06, against b18bf56.*

---

## 1. Architecture & Principles

1. **Per-Space Scoping**: Tiling geometry state (`TileSpace`) is maintained per monitor, per space (`MonitorState.tiling: Vec<TileSpace>`), holding slot insertion order and custom split ratios. Window floating state is window-scoped across all spaces and monitors (`SpaceManager.floating_windows`), session-scoped in memory, with `float_rules` as the persistent form.
2. **Opt-in & Graceful**: Disabled by default (`Config.tiling.enabled = false`). Can be globally toggled with `Ctrl+Alt+Shift+T` (`HOTKEY_ID_TILING_TOGGLE`), via IPC (`WM_WINSPACES_TILING_TOGGLE`), or in the settings window.
3. **Pure Math Core**: Geometry algorithms (`compute_dwindle`, `compute_master_stack`) and order reconciliation (`reconcile_order`) are pure functions residing in `winspaces-core::tiling` with comprehensive unit tests and zero Win32 FFI dependencies.
4. **Clean Win32 Placement**: Placements are committed in a single atomic batch via `BeginDeferWindowPos` / `DeferWindowPos` / `EndDeferWindowPos` with `SWP_NOZORDER | SWP_NOACTIVATE | SWP_FRAMECHANGED | SWP_NOCOPYBITS`.
5. **DWM Shadow Margin Compensation**: Compensates for Windows 10/11 invisible 7px drop-shadow borders (`DwmGetWindowAttribute(DWMWA_EXTENDED_FRAME_BOUNDS)`) so adjacent tiles fit truly pixel-flush against each other.
6. **Square Corner Rounding**: Sets `DWMWCP_DONOTROUND` (1) on tiled windows to prevent visual clipping artifacts at shared borders; restores `DWMWCP_DEFAULT` (0) when tiling is toggled off or a window is floated.

## 2. Layout Engine: Spiral Dwindle

The default layout algorithm (`LayoutKind::Dwindle`) recursively partitions available monitor work area:
- A single window occupies 100% of the work area.
- 2+ windows split each active partition along its longer dimension (horizontal split if width > height, vertical split otherwise) according to the partition's split ratio $r_i$ (default 0.5).
- **Split Orientation Override**: The primary split (split 0) can be forced side-by-side or stacked per space (`TileSpace.split_direction`), overriding the aspect-ratio heuristic. `Auto` (the default) keeps the aspect-ratio behavior; toggling resolves the currently effective direction and flips it. Deeper splits (1+) always remain aspect-driven — per-node control would require a full BSP tree, which the flat ratio list deliberately avoids.
- **Exactness Invariant**: Due to integer division, the right/bottom edge of the final tile in every split is anchored to the parent container's right/bottom boundary (`parent.right - gap`), ensuring zero single-pixel leaks or work area overflows.

## 3. Keyboard Control & Hotkeys

The default bindings (`Ctrl+Alt+Shift+T` toggles tiling, `Ctrl+Alt+Shift+arrows` move focus, `Ctrl+Shift+Win+arrows` swap tiles, `Ctrl+Alt+Shift+-`/`+` adjust the split ratio, `Ctrl+Alt+Shift+F` floats, `Ctrl+Alt+Shift+O` flips the split) are listed with every other shortcut in the [README](../README.md); the modifier masks and virtual-key codes behind them are the `default_tiling_*` functions in `crates/winspaces-common/src/config/defaults.rs`, and `decode_hotkey` in `crates/winspaces-core/src/hotkeys.rs` maps each registered id to its `HotkeyAction`.

Toggling the split orientation flashes a transient pill toast (`Split: Side by side` / `Split: Stacked`) on the affected monitor, reusing the space indicator surface.

There is no dedicated fullscreen hotkey: **maximize is the fullscreen mode** (see §5.5), so the native verbs already do the job — the maximize button, double-clicking the title bar, `Win+↑`, or dragging a window to the top edge. Restore (`Win+↓`, the restore button, or dragging the title bar) returns the window to its tile.

## 4. Mouse Interactions: Drag-Swap, Border Drag-Resize & Modifier Gestures

WinSpaces intercepts mouse move/size actions via `EVENT_SYSTEM_MOVESIZESTART` and `EVENT_SYSTEM_MOVESIZEEND` hooks:
1. **Drag-Active Protection**: When a tiled window drag begins (`MOVESIZESTART`), a drag-active marker is set on `SpaceManager`, causing any intermediate `flush_retile` calls on that space to skip so the window is never yanked out of the user's hand mid-gesture.
2. **Split 0 Border Resize**: Resizing a tile along the primary split boundary dynamically updates that space's split ratio (`ts.ratios[0]`), clamped to `[0.1, 0.9]`, and triggers a retile preserving the adjusted proportion.
3. **Tile Drag-Swap**: Dragging a tiled window and dropping it over another tile's area swaps their positions in the slot order (`ts.order`).
4. **Shift+Drag Split Toggle**: Holding `Shift` during a positional drag reroutes the gesture: a translucent accent-tinted ghost overlay previews the layout with the primary split orientation flipped, and the drop applies that toggle instead of a swap. Shift is the explicit opt-in — without it, drag-swap behaves exactly as before. The drop point does not matter (the intent is the modifier, not the target), and Shift never reroutes border drag-resizes, which keep adjusting the split ratio. Shift state is sampled by a poll timer that lives only for the duration of the drag, so the preview follows press/release in real time and the outcome always matches the preview shown at drop.
5. **Drag from Maximized**: Windows restores a maximized window the moment its title bar is dragged. The tiler records `from_maximized` at `MOVESIZESTART` so the size delta of that restore is never mistaken for a border resize; the drop then classifies as usual — over another tile swaps slots, anywhere else snaps back into its own slot, with `Shift` toggling the split.
6. **Forgiving Snap-Back**: Ambiguous motions, deep-split border adjustments, or drops outside the tiling area automatically snap back to the computed layout on mouse release (`MOVESIZEEND`).
7. **Floating & Non-Tiled Isolation**: Floating windows, pinned windows, and windows on non-tiled spaces are completely untouched by the drag classifier.

## 5. Tiling Lifecycle & Hazards

### 5.1 Inactive Space Safety
Only visible spaces on active monitors (`mon.current == space_idx`) are tiled during `flush_retile`. Background spaces are marked dirty (`ts.dirty = true`) and retiled immediately when brought to focus via `switch_space`.

### 5.2 Workspace Restore Fight Prevention
`SpaceManager::try_enforce_restore`, `enforce_restore_pass`, and `heal_restored_placement` early-return whenever `tiling_owns_window(hwnd)` is true to prevent layout restoration passes from fighting with the dynamic tiler.

### 5.3 Asynchronous Debouncing
Producers (window creation, destruction, minimize, space switches) mark the target space `dirty` and call `schedule_retile()`. This posts `WM_WINSPACES_RETILE` to the daemon message window, which sets a coalescable timer (`TIMER_RETILE`, 50ms). Retile flushing runs in a fresh message pump iteration.

### 5.4 Resistance Detection & Auto-Floating
After retiling, `TIMER_RETILE_VERIFY` (200ms) runs a verification sweep comparing actual `DWMWA_EXTENDED_FRAME_BOUNDS` with expected target bounds (8 px of tolerance for min-size clamps and frame rounding). A window that sits on its slot but came out **larger** than it (position within tolerance, width/height at or above the target) is clamped by its own minimum size, not resisting: it is left overflowing its tile (`TileSpace.overflowing`, logged once) rather than floated, the same way Hyprland treats min-size windows. A window that resists in any other way (moved itself, shrank, or did not move at all — elevated processes under UIPI) across 4 consecutive sweeps is automatically marked floating and logged.

Auto-floats are recorded in `SpaceManager.auto_floated` as well as `floating_windows`. A float made with the toggle hotkey is a decision and stays until toggled back or the window closes; an auto-float is a measurement taken under one layout and is **re-admitted** (removed from both sets, space marked dirty) when that layout is gone: the window is tracked onto a different monitor or space (`track_window`), or its space loses a tile so every slot grows (`flush_retile` compares the candidate count with the previous non-auto-floated slot count). If it resists again it strikes out again after four sweeps. Toggling float on an auto-floated window converts it into a user decision either way.

### 5.5 Maximize as Fullscreen Mode
A maximized window that already holds a slot in the space's `order` is **honoured**, not flattened: `flush_retile` keeps its slot reserved (its rect stays in `expected`, so drag targets and focus navigation still see the full layout), computes the other tiles as if it were in place, and simply excludes it from `apply_layout` — pushing it would un-maximize it. It sits over the layout until the user restores it. Because the tiler placed the window with `DeferWindowPos` before it was maximized, its `rcNormalPosition` *is* its slot, so restoring lands it back in the layout without any daemon intervention. If the layout moved underneath while it was maximized (a window opened or closed), an `EVENT_OBJECT_LOCATIONCHANGE` hook (`tiling_on_window_restored`, a set probe on the honoured windows) marks the space dirty on restore so the window is pushed to its current slot. The record of honoured windows per space is `TileSpace.maximized`, rebuilt on every flush; `verify_retile` treats an honoured window sitting over its slot as correct rather than as a flatten strike.

Only **newcomers** to a space are flattened: a window that arrives maximized (browsers and Explorer remember the state) is un-maximized without activating (`SW_SHOWNOACTIVATE`, animations suppressed via `AnimationGuard`) and pulled into the layout, so it joins the tiles instead of landing on top of them. A newcomer that refuses to un-maximize after 4 attempts (elevated processes under UIPI) is auto-floated. Floating and sticky windows never have their maximize state touched.

## 6. Window Float Rules

Specific applications can be permanently exempted from dynamic tiling via `FloatRule` entries stored in `settings.json` under `tiling.float_rules`:

- **Identity-Based Matching**: `FloatRule` shares the identification subset of `WorkspaceRule` (`name`, `aumid`, `exe_path`, `class_name`, `title_pattern`).
- **Adapter Reuse**: Matching evaluates rule specificity using the pure `score_rule` matcher via `FloatRule::as_workspace_rule()`, avoiding duplicate matching logic.
- **Persistence**: Unlike in-session temporary floats (which are session-scoped and cleared on restart), `FloatRule` configurations persist permanently across daemon restarts.

## 7. CLI & Inter-Process Control

- **CLI Flag**: `winspaces.exe --tiling-toggle` (or `winspaces.exe -t`) finds the running daemon message window and posts `WM_WINSPACES_TILING_TOGGLE`.
- **UIPI Exemption**: `WM_WINSPACES_TILING_TOGGLE` is explicitly registered in `ChangeWindowMessageFilterEx` (`MSGFLT_ALLOW`), permitting medium-integrity shell scripts or hotkey daemons to toggle tiling even when the daemon runs elevated.

## 8. Mission Control Integration

When dynamic tiling is enabled, Mission Control displays a subtle `• Tiled` indicator in the top Spaces bar on each space card (e.g. `Active • 3 windows • Tiled`). Dropping a window card onto a tiled space automatically re-homes and integrates the window into that space's dwindle spiral hierarchy.

## 9. Persistence

Per-space tiling state (split ratios, split orientation overrides, in-session floating sets, slot order) is session-state — it survives display topology changes and RDP reconnects in-session (carried across `handle_display_change` keyed by stable monitor id), but is NOT persisted across daemon restarts. The global enable flag, inner/outer gaps, hotkey assignments, and configured `float_rules` survive restart via `Config.tiling` in `settings.json`. After a restart, tiled spaces re-tile in tracked order with default 0.5 ratios and `Auto` split orientation on the first flush.

## 10. Known Limitations

1. **Elevated Windows**: When the WinSpaces daemon runs non-elevated (default), User Interface Privilege Isolation (UIPI) prevents `DeferWindowPos` from resizing elevated admin windows. The verify sweep will detect resistance and auto-float them. Run the daemon elevated (`.\dev run --admin`) to manage elevated windows.
2. **Single Layout Algorithm**: Version 1 implements the dynamic BSP spiral dwindle layout. Master-stack layout is reserved for future milestones.
3. **Global Toggle**: Dynamic tiling is toggled globally across all managed monitors and spaces. Per-space opt-out is achieved via per-window float rules or in-session `toggle_float`.
4. **Maximize Is Per Window**: Several tiles on one space can be maximized at once (each is honoured independently); the tiler does not arbitrate a single fullscreen window per space the way Hyprland does. Alt+Tab moves between them as usual.
5. **Adding Float Rules**: The native Settings window allows reviewing and deleting existing `float_rules`, but provides no UI affordance for adding new rules. New float rules are currently configured by manually editing `settings.json` under `tiling.float_rules`.

## See also

- [`dwm.md`](dwm.md) §2 for the shadow margins the tiler compensates.
- [`mission-control.md`](mission-control.md) for the overlay's tiled badge and drop targets.
- [`display-topology.md`](display-topology.md) for what survives a monitor or RDP change.
- [`../reports/tiling-roadmap.md`](../reports/tiling-roadmap.md) for the features parked and skipped.
