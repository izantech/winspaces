# Dynamic Tiling Window Manager

WinSpaces includes an optional Hyprland-inspired dynamic tiling engine. When enabled, managed windows on each space are arranged automatically in a BSP / spiral dwindle hierarchy with zero overlapping frames and configurable inner/outer gaps.

## Architecture & Principles

1. **Per-Space Scoping**: Tiling geometry state (`TileSpace`) is maintained per monitor, per space (`MonitorState.tiling: Vec<TileSpace>`), holding slot insertion order and custom split ratios. Window floating state is window-scoped across all spaces and monitors (`SpaceManager.floating_windows`), session-scoped in memory, with `float_rules` as the persistent form.
2. **Opt-in & Graceful**: Disabled by default (`Config.tiling.enabled = false`). Can be globally toggled with `Ctrl+Alt+Shift+T` (`HOTKEY_ID_TILING_TOGGLE`), via IPC (`WM_WINSPACES_TILING_TOGGLE`), or in the settings window.
3. **Pure Math Core**: Geometry algorithms (`compute_dwindle`, `compute_master_stack`) and order reconciliation (`reconcile_order`) are pure functions residing in `winspaces-core::tiling` with comprehensive unit tests and zero Win32 FFI dependencies.
4. **Clean Win32 Placement**: Placements are committed in a single atomic batch via `BeginDeferWindowPos` / `DeferWindowPos` / `EndDeferWindowPos` with `SWP_NOZORDER | SWP_NOACTIVATE | SWP_FRAMECHANGED | SWP_NOCOPYBITS`.
5. **DWM Shadow Margin Compensation**: Compensates for Windows 10/11 invisible 7px drop-shadow borders (`DwmGetWindowAttribute(DWMWA_EXTENDED_FRAME_BOUNDS)`) so adjacent tiles fit truly pixel-flush against each other.
6. **Square Corner Rounding**: Sets `DWMWCP_DONOTROUND` (1) on tiled windows to prevent visual clipping artifacts at shared borders; restores `DWMWCP_DEFAULT` (0) when tiling is toggled off or a window is floated.

## Layout Engine: Spiral Dwindle

The default layout algorithm (`LayoutKind::Dwindle`) recursively partitions available monitor work area:
- A single window occupies 100% of the work area.
- 2+ windows split each active partition along its longer dimension (horizontal split if width > height, vertical split otherwise) according to the partition's split ratio $r_i$ (default 0.5).
- **Exactness Invariant**: Due to integer division, the right/bottom edge of the final tile in every split is anchored to the parent container's right/bottom boundary (`parent.right - gap`), ensuring zero single-pixel leaks or work area overflows.

## Keyboard Control & Hotkeys

| Action | Default Shortcut | Modifiers / VK | Description |
| :--- | :--- | :--- | :--- |
| **Toggle Tiling** | `Ctrl+Alt+Shift+T` | `0x7` / `0x54` | Enables or disables tiling globally |
| **Focus Left / Right** | `Ctrl+Alt+Shift+← / →` | `0x7` / `0x25, 0x27` | Moves focus to adjacent left/right tile |
| **Focus Up / Down** | `Ctrl+Alt+Shift+↑ / ↓` | `0x7` / `0x26, 0x28` | Moves focus to adjacent upper/lower tile |
| **Swap Left / Right** | `Ctrl+Shift+Win+← / →` | `0xE` / `0x25, 0x27` | Swaps active tile with neighbor in slot order |
| **Swap Up / Down** | `Ctrl+Shift+Win+↑ / ↓` | `0xE` / `0x26, 0x28` | Swaps active tile with neighbor in slot order |
| **Ratio Shrink / Grow** | `Ctrl+Alt+Shift+- / +` | `0x7` / `0xBD, 0xBB` | Adjusts split ratio by step % (default 5%) |
| **Toggle Float** | `Ctrl+Alt+Shift+F` | `0x7` / `0x46` | Floats or un-floats active window |

## Mouse Interactions: Drag-Swap & Border Drag-Resize

WinSpaces intercepts mouse move/size actions via `EVENT_SYSTEM_MOVESIZESTART` and `EVENT_SYSTEM_MOVESIZEEND` hooks:
1. **Drag-Active Protection**: When a tiled window drag begins (`MOVESIZESTART`), a drag-active marker is set on `SpaceManager`, causing any intermediate `flush_retile` calls on that space to skip so the window is never yanked out of the user's hand mid-gesture.
2. **Split 0 Border Resize**: Resizing a tile along the primary split boundary dynamically updates that space's split ratio (`ts.ratios[0]`), clamped to `[0.1, 0.9]`, and triggers a retile preserving the adjusted proportion.
3. **Tile Drag-Swap**: Dragging a tiled window and dropping it over another tile's area swaps their positions in the slot order (`ts.order`).
4. **Forgiving Snap-Back**: Ambiguous motions, deep-split border adjustments, or drops outside the tiling area automatically snap back to the computed layout on mouse release (`MOVESIZEEND`).
5. **Floating & Non-Tiled Isolation**: Floating windows, pinned windows, and windows on non-tiled spaces are completely untouched by the drag classifier.

## Tiling Lifecycle & Hazards



### 1. Inactive Space Safety
Only visible spaces on active monitors (`mon.current == space_idx`) are tiled during `flush_retile`. Background spaces are marked dirty (`ts.dirty = true`) and retiled immediately when brought to focus via `switch_space`.

### 2. Workspace Restore Fight Prevention
`SpaceManager::try_enforce_restore`, `enforce_restore_pass`, and `heal_restored_placement` early-return whenever `tiling_owns_window(hwnd)` is true to prevent layout restoration passes from fighting with the dynamic tiler.

### 3. Asynchronous Debouncing
Producers (window creation, destruction, minimize, space switches) mark the target space `dirty` and call `schedule_retile()`. This posts `WM_WINSPACES_RETILE` to the daemon message window, which sets a coalescable timer (`TIMER_RETILE`, 50ms). Retile flushing runs in a fresh message pump iteration.

### 4. Resistance Detection & Auto-Floating
After retiling, `TIMER_RETILE_VERIFY` (200ms) runs a verification sweep comparing actual `DWMWA_EXTENDED_FRAME_BOUNDS` with expected target bounds (tolerance 2px). If a window resists resizing (e.g. min-size constraints or elevated processes) across 2 consecutive sweeps, it is automatically marked floating (`self.floating_windows.insert(hwnd)`) and logged.

### 5. Maximized Window Flattening
Dynamic tiling flattens maximize: any maximized tileable window on an enabled, visible space is automatically un-maximized without activating (`SW_SHOWNOACTIVATE`) and pulled into the layout on retile flush (with OS restore animations suppressed via `AnimationGuard`). The escape hatch for keeping a window maximized is to float it first (`Ctrl+Alt+Shift+F` or configured `float_rules`) — floating and sticky windows never have their maximize state touched.

## Window Float Rules

Specific applications can be permanently exempted from dynamic tiling via `FloatRule` entries stored in `settings.json` under `tiling.float_rules`:

- **Identity-Based Matching**: `FloatRule` shares the identification subset of `WorkspaceRule` (`name`, `aumid`, `exe_path`, `class_name`, `title_pattern`).
- **Adapter Reuse**: Matching evaluates rule specificity using the pure `score_rule` matcher via `FloatRule::as_workspace_rule()`, avoiding duplicate matching logic.
- **Persistence**: Unlike in-session temporary floats (which are session-scoped and cleared on restart), `FloatRule` configurations persist permanently across daemon restarts.

## CLI & Inter-Process Control

- **CLI Flag**: `winspaces.exe --tiling-toggle` (or `winspaces.exe -t`) finds the running daemon message window and posts `WM_WINSPACES_TILING_TOGGLE`.
- **UIPI Exemption**: `WM_WINSPACES_TILING_TOGGLE` is explicitly registered in `ChangeWindowMessageFilterEx` (`MSGFLT_ALLOW`), permitting medium-integrity shell scripts or hotkey daemons to toggle tiling even when the daemon runs elevated.

## Mission Control Integration

When dynamic tiling is enabled, Mission Control displays a subtle `• Tiled` indicator in the top Spaces bar on each space card (e.g. `Active • 3 windows • Tiled`). Dropping a window card onto a tiled space automatically re-homes and integrates the window into that space's dwindle spiral hierarchy.

## Persistence

Per-space tiling state (split ratios, in-session floating sets, slot order) is session-state — it survives display topology changes and RDP reconnects in-session (carried across `handle_display_change` keyed by stable monitor id), but is NOT persisted across daemon restarts. The global enable flag, inner/outer gaps, hotkey assignments, and configured `float_rules` survive restart via `Config.tiling` in `settings.json`. After a restart, tiled spaces re-tile in tracked order with default 0.5 ratios on the first flush.

## Known Limitations

1. **Elevated Windows**: When the WinSpaces daemon runs non-elevated (default), User Interface Privilege Isolation (UIPI) prevents `DeferWindowPos` from resizing elevated admin windows. The verify sweep will detect resistance and auto-float them. Run the daemon elevated (`.\dev run --admin`) to manage elevated windows.
2. **Single Layout Algorithm**: Version 1 implements the dynamic BSP spiral dwindle layout. Master-stack layout is reserved for future milestones.
3. **Global Toggle**: Dynamic tiling is toggled globally across all managed monitors and spaces. Per-space opt-out is achieved via per-window float rules or in-session `toggle_float`.
4. **Maximized State on Tiled Windows**: Maximizing a tiled window is flattened on the next re-layout flush; float the window first to keep it maximized over the tiles.
5. **Adding Float Rules**: The native Settings window allows reviewing and deleting existing `float_rules`, but provides no UI affordance for adding new rules. New float rules are currently configured by manually editing `settings.json` under `tiling.float_rules`.

