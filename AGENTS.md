# AGENTS.md

Guidance for AI agents working in this repository.

## Project

WinSpaces is a per-monitor independent virtual desktop manager for Windows, written in Rust against raw Win32 (`windows-sys`). Unlike Windows' built-in virtual desktops (which move all monitors together), each display gets its own independent set of spaces — like macOS "Displays have separate Spaces".

**Windows-only.** The crate uses `#![windows_subsystem = "windows"]` and raw `windows-sys` FFI; it will not compile on non-Windows targets.

## Build & Run

A `dev` task runner (`dev.ps1` + `dev.cmd` shim) wraps these — e.g. `dev build --release`, `dev check` (fmt + clippy + test), `dev recover`. Run `dev help` for the full list.

```sh
cargo build --release      # size-optimized → target/release/winspaces.exe
cargo check                # fast type-check
cargo clippy -- -D warnings
```

- The release profile (`opt-level = "z"`, LTO, single codegen unit, `panic = "abort"`, stripped) is deliberately size-tuned. Don't loosen it casually — the tiny footprint is a stated feature.
- There are **no tests** in this crate.
- It runs as a background tray app with no console. Debugging is via the log file, not stdout.

## Runtime artifacts

On launch WinSpaces reads/writes (portable mode wins if `settings.json` exists next to the `.exe`):
- Config: `%LOCALAPPDATA%\WinSpaces\settings.json`
- Log: `%LOCALAPPDATA%\WinSpaces\winspaces.log` (written via `log_info!` / `log_warn!` / `log_error!` macros from `logger.rs`)

`scripts/recover-windows.ps1` is a recovery tool: if a buggy build leaves windows cloaked/hidden after exit, run it to uncloak every top-level window and re-show the ones WinSpaces was tracking. Safe to re-run.

## Architecture

### The virtual-desktop illusion (core design)

WinSpaces does **not** use the native Windows virtual-desktop COM API. It keeps its own per-desktop window membership (`MonitorState::desktops: [Vec<HWND>; NUM_DESKTOPS]`, where `NUM_DESKTOPS = 4`) and physically hides/shows windows to create the appearance of separate desktops per monitor. Most design consequences flow from this: z-order must be saved/restored on switch (`restore_zorder`), hidden windows must be uncloaked on exit (`windows_show_all`), and hidden-window activation must auto-switch desktops (the foreground hook).

### Process model & state

Single-threaded classic Win32. `main.rs` creates a hidden message-only window, registers global hotkeys, optionally installs a foreground WinEvent hook, and runs a `GetMessageW` loop that handles `WM_HOTKEY` inline and dispatches the rest. All live state lives in a thread-local `APP_STATE: RefCell<Option<AppState>>`. Window procs and the event-hook callback reach it via `APP_STATE.with(|s| s.try_borrow_mut()...)` — `try_borrow` is deliberate because callbacks can re-enter the borrow (a visibility change can fire another foreground event).

### Two taskbar modes (`show_all_taskbar`)

This boolean (`config.rs`, toggled from the tray menu) changes the hiding strategy and is the main behavioral switch:
- `true` (default): hide via `SW_FORCEMINIMIZE` — windows minimize but remain on the taskbar. The foreground WinEvent hook is installed so activating a minimized window auto-switches its desktop (`foreground_hook_proc` in `main.rs`).
- `false`: hide via `SWP_HIDEWINDOW`/`SW_HIDE` plus DWM cloaking (`DWMWA_CLOAK`) — windows vanish from the taskbar entirely. No foreground hook.

`desktop.rs::set_window_visibility` is the single chokepoint implementing both modes.

### Per-window state via Win32 properties

Instead of a map, per-window bookkeeping is stored on the window itself via `SetPropA`/`GetPropA` under the key `"WinSpacesWindowState"` (`desktop.rs`) — a bitmask (`TRACKED | WAS_ICONIC | FORCED_MINIMIZED | CLOAKED`). The recovery script reads and clears this same property. If you rename it or change the bits, update `scripts/recover-windows.ps1` too.

### Hotkeys

`HotkeyManager` (`hotkeys.rs`) uses thread-targeted `RegisterHotKey` with `MOD_NOREPEAT`. IDs are contiguous ranges: switch (`0..N`), move (`N..2N`), then special (EXIT, TOGGLE, PREV, NEXT, MOVE_PREV, MOVE_NEXT). **EXIT (`Alt+Ctrl+Shift+Q`) and TOGGLE (`Alt+Ctrl+Shift+S`) are hardcoded, not configurable.** Registration is **all-or-nothing**: on any conflict the whole set rolls back and returns `false`; callers react by keeping the prior config and/or opening the GUI.

### Configuration & GUI

`Config` is plain serde JSON; modifiers are raw Win32 bitmasks (`MOD_ALT=0x1`, `MOD_CONTROL=0x2`, `MOD_SHIFT=0x4`, `MOD_WIN=0x8`), masked to the valid set on load (`sanitize_modifiers`). `ConfigWindow` (`gui.rs`) is a hand-built Win32 dialog (no framework) that captures hotkeys by listening for `WM_KEYDOWN`/`WM_SYSKEYDOWN` and requiring ≥1 modifier; it edits a `pending_config` until Apply, which routes through `main::apply_config` (re-registers hotkeys, persists, rolls back on failure).

### Pervasive `unsafe`

Nearly all Win32 calls are `unsafe` (raw `windows-sys`, not the higher-level `windows` crate). When adding FFI, follow the existing pattern of checking return codes and guarding with `IsWindow` / `is_valid_window` before touching an `HWND`, since handles may be invalidated between events.
