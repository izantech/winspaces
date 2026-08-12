# AGENTS.md

Guidance for AI agents working in this repository.

## Project

WinSpaces is a per-monitor independent virtual desktop manager for Windows. Unlike Windows' built-in virtual desktops (which move all monitors together), each display gets its own independent set of spaces — like macOS "Displays have separate Spaces".

**Windows-only.** A single Rust binary built on raw Win32 FFI (`windows-sys`): the background daemon and the native Fluent settings window both live in `winspaces.exe`.

## Architecture & Technology Stack

WinSpaces is one native binary, `winspaces.exe`, built from five crates in strict one-way dependency order — nothing depends upward:

```
winspaces        (bin)  -> ui, core, common
winspaces-ui             -> win32, core, common
winspaces-core           -> win32, common
winspaces-win32          -> common
winspaces-common
```

- **`winspaces-common`**: the config/layout schema (`Config`, `WorkspaceRule`, `LayoutStore`/`TopologySnapshot`), the Win32 IPC message constants, and the logger. Single source of truth — the daemon and the settings window are the same binary, so schema changes happen in exactly one place.
- **`winspaces-win32`**: safe-ish FFI kit with no product knowledge — GDI/DWM/DPI drawing primitives, window-class registration, the low-level keyboard/foreground hooks, and the ImmersiveShell shell-cloak COM surface.
- **`winspaces-core`**: the daemon's non-UI logic — per-monitor desktop/space tracking and the window show/hide state machine, workspace-rule capture/matching/placement, display topology identity, and global hotkeys. No rendering, no `AppState`.
- **`winspaces-ui`**: the four owner-drawn surfaces — tray icon + acrylic context menu, Mission Control, the settings window, and the transient space indicator — as peers sharing one crate-level theme and the `winspaces-win32` drawing kit.
- **`winspaces`** (bin): CLI dispatch, the message loop, `AppState`, and the wiring that lets the crates below act on daemon state they cannot otherwise reach (see "Mission Control's host indirection" below).

See [`docs/crate-layout.md`](docs/crate-layout.md) for the full per-crate breakdown.

Runtime-wise the binary still plays two process roles:

1. **The daemon** (default invocation, < 3 MB RAM, ~460 KB binary):
   - Manages desktop window membership, window hiding via DWM cloaking and the ImmersiveShell shell cloak (`docs/dwm.md` §5), a 32-bit ARGB Fluent tray icon, a custom acrylic tray context menu (hand-drawn `WS_POPUP` flyout with DWM backdrop, Fluent glyphs, LL-hook light dismiss; classic OS-themed `HMENU` fallback pre-Win11), and global hotkeys.
   - **Mission Control**: native GPU-accelerated Exposé overlay with live DWM thumbnails (`DwmRegisterThumbnail`), native aspect-ratio preservation (`DwmQueryThumbnailSourceSize`), top Spaces bar, drag-and-drop space reordering (or `Ctrl+Shift+←/→`), and drag-and-drop window relocation across spaces. It lives in `winspaces-ui` but never sees `AppState`: its entry points take `&mut DesktopManager` directly, and anything it cannot do itself — adding/removing/reordering spaces, switching a space, moving a window — goes through an `McHost` vtable of plain `fn` pointers that the bin installs at startup. Fn pointers, not posted messages: a drop completes the reorder and the overlay refresh synchronously before `WM_LBUTTONUP` returns, and deferring either through `PostMessage` would change the frame the overlay repaints in. See [`docs/mission-control.md`](docs/mission-control.md).
   - **Interception & Triggers**: single left-click on the tray icon toggles Mission Control; `WH_KEYBOARD_LL` intercepts `Win+Tab`; `winspaces.exe --mission-control` sends the toggle IPC message; `--exit` / `--kill` gracefully stops the running daemon.
   - **Taskbar & App Activation**: `EVENT_SYSTEM_FOREGROUND` and `ShellHook` (`HSHELL_WINDOWACTIVATED` / `HSHELL_RUDEAPPACTIVATED`) intercept taskbar clicks and app activations, automatically switching the target display to that window's desktop space.
   - **Window Lifecycle**: automatic desktop window scanning on startup and Mission Control open; windows are untracked on `HSHELL_WINDOWDESTROYED` (guarded on real liveness, because the shell also fires it when our own cloaking removes a window from its list) with a prune pass on each scan as backstop; eligibility is decided structurally (Alt-Tab-style owner-chain walk, extended styles, shell class blacklist, cloak state — no title matching), filtering out shell hosts, IME windows, and system-cloaked services. On startup and clean exit the daemon reclaims windows still carrying WinSpaces `SetProp` state (crash recovery), and `WM_DISPLAYCHANGE` re-maps per-monitor space state by display device name on monitor hotplug.
   - **Space Indicator**: a transient click-through "Space N" panel near the taskbar of the display that just switched, on every trigger (hotkeys, tray, Mission Control, taskbar/app activation). Hooked at `DesktopManager::switch_desktop` through a `fn`-pointer observer the bin installs, so no trigger can forget to notify; the only layered, backdrop-free surface in the codebase, because it is the only one that has to fade (`docs/space-indicator.md`).
   - **Workspaces**: multi-monitor window layout capture and automatic rule-based placement on startup.
   - Listens for IPC reload (`WM_USER + 100`), capture (`WM_USER + 101`), restore (`WM_USER + 102`), and Mission Control (`WM_USER + 103`) messages.

2. **The native settings window** (`winspaces.exe --settings`):
   - Windows 11 Settings-style configurator, hand-drawn with the same GDI+DWM recipe as the tray menu (`docs/settings-ui.md`): real Mica backdrop, nav rail, Fluent cards, toggles, theme combo, hotkey recorder — all owner-drawn regions of one window, no UI framework.
   - Runs as a **separate process instance** of the same exe (spawned by the tray "Settings" item); a settings crash can never take the daemon down, and the daemon pays zero runtime cost for the settings code while it's closed.
   - The tray menu and the settings window are peers in `winspaces-ui`, both drawing on the shared `winspaces-win32` kit and a shared crate-level `theme` module — the menu no longer reaches into a settings-owned theme, and settings no longer calls back into the menu for window setup, which used to be a real module cycle.
   - Colors come only from that shared theme's palette tokens — Light/Dark/system via the in-app "App Theme" selector (persisted at `HKCU\Software\WinSpaces\GuiTheme`; never part of the daemon config contract), with high-contrast fallback.
   - Reads/writes `%LOCALAPPDATA%\WinSpaces\settings.json` via `winspaces_common::Config` (single source of truth — schema, defaults, and normalization live only in `crates/winspaces-common`) and posts Win32 IPC reload messages.
   - Manages the HKCU `Run` autostart entry for the daemon.

### Crate layering

- Dependency order is `winspaces (bin) -> winspaces-ui -> winspaces-core -> winspaces-win32 -> winspaces-common`; a crate may only depend on crates at or below its own position in that list, never above.
- Every crate declares every `windows-sys` feature it actually uses in its own `Cargo.toml` — never rely on a sibling crate having enabled a feature you need. A workspace-wide build unifies features across crates, so a missing declaration compiles silently in the workspace and only breaks when that crate is built or reused in isolation (`cargo check -p <crate>` is the way to catch it).

## Build & Run

A `dev` task runner (`dev.ps1` + `dev.cmd` shim) wraps all build and execution tasks:

```powershell
.\dev build             # Builds the Rust workspace (daemon + settings window)
.\dev run               # Launches the daemon asynchronously (inherits terminal integrity; non-elevated is default)
.\dev run --admin       # Launches the daemon elevated (prompts UAC if terminal is non-elevated)
.\dev run settings      # Opens the native settings window (winspaces.exe --settings)
.\dev check             # Runs fmt + clippy + test checks
.\dev dist              # Builds the signed-if-configured installer into dist\
```

## Documentation

Architecture specifications and technical references (in `kebab-case`):
- [`docs/dwm.md`](docs/dwm.md): DWM margins, snapping mathematics, AUMID identification, window placement, and the DWM cloaking design (mechanism, crash-recovery contract, rejected alternatives).
- [`docs/mission-control.md`](docs/mission-control.md): Mission Control architecture, system shortcut interception (including low-level hook constraints), and window filtering.
- [`docs/tray-and-menu.md`](docs/tray-and-menu.md): Tray badge icon generation and the custom acrylic context menu (DWM backdrop recipe, alpha-managed GDI rendering, hook-based dismissal, and why it stays lightweight).
- [`docs/settings-ui.md`](docs/settings-ui.md): The native settings window — Mica variant of the menu recipe, owner-drawn control kit, hotkey-recorder hook design, theming.
- [`docs/space-indicator.md`](docs/space-indicator.md): The "Space N" switch indicator — the core-level observer hook that covers every trigger, and why this one surface is layered instead of DWM-backdropped.
- [`docs/display-topology.md`](docs/display-topology.md): Stable monitor identity (`QueryDisplayConfig` device paths vs `\\.\DISPLAYn` slots), RDP topology teardown, the debounced reconcile, and per-topology layout shadow/restore.
- [`docs/ipc-and-config.md`](docs/ipc-and-config.md): Win32 IPC protocol (message window, `WM_USER` messages, UIPI filter), CLI flags, the `settings.json` and `layouts.json` schemas + normalization contract, and the elevation posture.
- [`docs/benchmarks.md`](docs/benchmarks.md): How to measure the daemon's cost (message-driven harnesses, the A/B protocol, cache-vs-leak) and the latest results. The single home for performance numbers — other pages link here rather than repeat them.
- [`docs/distribution.md`](docs/distribution.md): Inno Setup installer, code signing, and the release/update flow (`dev dist`, `.github/workflows/release.yml`).
- [`docs/crate-layout.md`](docs/crate-layout.md): The five-crate dependency graph, what belongs in each crate, and the per-crate `windows-sys` feature rule.

## Runtime Artifacts

On launch WinSpaces reads/writes (portable mode wins if `settings.json` exists next to the `.exe`):
- Config: `%LOCALAPPDATA%\WinSpaces\settings.json`
- Log: `%LOCALAPPDATA%\WinSpaces\winspaces.log` (written via `winspaces-common`'s `Logger::log`)

`scripts/recover-windows.ps1` is a recovery tool: if a buggy build leaves windows cloaked/hidden after exit, run it to uncloak every top-level window and re-show the ones WinSpaces was tracking. Safe to re-run.
