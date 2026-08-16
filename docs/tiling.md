# Dynamic Tiling Window Manager

WinSpaces includes an optional Hyprland-inspired dynamic tiling engine. When enabled, managed windows on each space are arranged automatically in a BSP / spiral dwindle hierarchy with zero overlapping frames and configurable inner/outer gaps.

## Architecture & Principles

1. **Per-Space Scoping**: Tiling state (`TileSpace`) is maintained per monitor, per space (`MonitorState.tiling: Vec<TileSpace>`). Each space maintains its own insertion order, custom split ratios, and floating set.
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


## Tiling Lifecycle & Hazards

### 1. Inactive Space Safety
Only visible spaces on active monitors (`mon.current == space_idx`) are tiled during `flush_retile`. Background spaces are marked dirty (`ts.dirty = true`) and retiled immediately when brought to focus via `switch_space`.

### 2. Workspace Restore Fight Prevention
`SpaceManager::try_enforce_restore`, `enforce_restore_pass`, and `heal_restored_placement` early-return whenever `tiling_owns_window(hwnd)` is true to prevent layout restoration passes from fighting with the dynamic tiler.

### 3. Asynchronous Debouncing
Producers (window creation, destruction, minimize, space switches) mark the target space `dirty` and call `schedule_retile()`. This posts `WM_WINSPACES_RETILE` to the daemon message window, which sets a coalescable timer (`TIMER_RETILE`, 50ms). Retile flushing runs in a fresh message pump iteration.

### 4. Resistance Detection & Auto-Floating
After retiling, `TIMER_RETILE_VERIFY` (200ms) runs a verification sweep comparing actual `DWMWA_EXTENDED_FRAME_BOUNDS` with expected target bounds (tolerance 2px). If a window resists resizing (e.g. min-size constraints or elevated processes) across 2 consecutive sweeps, it is automatically marked floating (`ts.floating.insert(hwnd)`) and logged.
