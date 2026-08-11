# AGENTS.md

Guidance for AI agents working in this repository.

## Project

WinSpaces is a per-monitor independent virtual desktop manager for Windows. Unlike Windows' built-in virtual desktops (which move all monitors together), each display gets its own independent set of spaces — like macOS "Displays have separate Spaces".

**Windows-only.** A single Rust binary built on raw Win32 FFI (`windows-sys`): the background daemon and the native Fluent settings window both live in `winspaces.exe`.

## Architecture & Technology Stack

The project is decoupled into two clean boundaries (one binary, two process roles):

1. **Rust Daemon (`crates/winspaces-daemon`, `winspaces.exe`)**:
   - Ultra-fast, size-optimized background process (< 3 MB RAM, ~200 KB binary).
   - Manages desktop window membership, window hiding via DWM cloaking and the ImmersiveShell shell cloak (`shell_cloak.rs`, `docs/dwm.md` §5), 32-bit ARGB Fluent tray icon, custom acrylic tray context menu (`menu.rs`: hand-drawn `WS_POPUP` flyout with DWM backdrop, Fluent glyphs, LL-hook light dismiss, and light/dark palette resolved per open from `settings_ui::theme`; classic OS-themed `HMENU` fallback pre-Win11), and global hotkeys.
   - **Mission Control (`mission_control.rs`)**: Native GPU-accelerated Exposé overlay with live DWM thumbnails (`DwmRegisterThumbnail`), native aspect-ratio preservation (`DwmQueryThumbnailSourceSize`), top Spaces bar, drag-and-drop space reordering (or `Ctrl+Shift+←/→`), and drag-and-drop window relocation across spaces.
   - **Interception & Triggers**: Single left-click on Tray icon toggles Mission Control; `WH_KEYBOARD_LL` hook intercepts `Win+Tab`; CLI switch `winspaces.exe --mission-control` sends `WM_WINSPACES_TOGGLE_MISSION_CONTROL` IPC; CLI flag `winspaces.exe --exit` / `--kill` gracefully stops the running background daemon.
   - **Taskbar & App Activation (`main.rs`)**: `EVENT_SYSTEM_FOREGROUND` and `ShellHook` (`HSHELL_WINDOWACTIVATED` / `HSHELL_RUDEAPPACTIVATED`) intercept taskbar clicks and app activations, automatically switching the target display to that window's desktop space.
   - **Window Lifecycle (`desktop.rs`)**: Automatic desktop window scanning on startup and Mission Control open; eligibility is decided structurally (Alt-Tab-style owner-chain walk, extended styles, shell class blacklist, cloak state — no title matching), filtering out shell hosts, IME windows, and system-cloaked services. On startup and clean exit the daemon reclaims windows still carrying WinSpaces `SetProp` state (crash recovery), and `WM_DISPLAYCHANGE` re-maps per-monitor space state by display device name on monitor hotplug.
   - **Workspaces (`workspaces.rs`)**: Multi-monitor window layout capture and automatic rule-based placement on startup.
   - Listens for IPC reload (`WM_USER + 100`), capture (`WM_USER + 101`), restore (`WM_USER + 102`), and Mission Control (`WM_USER + 103`) messages.

2. **Native Settings Window (`crates/winspaces-daemon/src/settings_ui/`, `winspaces.exe --settings`)**:
   - Windows 11 Settings-style configurator, hand-drawn with the same GDI+DWM recipe as the tray menu (`docs/settings-ui.md`): real Mica backdrop, nav rail, Fluent cards, toggles, theme combo, hotkey recorder — all owner-drawn regions of one window, no UI framework.
   - Runs as a **separate process instance** of the daemon exe (spawned by the tray "Settings" item); a settings crash can never take the daemon down, and the daemon pays zero runtime cost for the settings code while it's closed.
   - Colors come only from `settings_ui/theme.rs` palette tokens — Light/Dark/system via the in-app "App Theme" selector (persisted at `HKCU\Software\WinSpaces\GuiTheme`; never part of the daemon config contract), with high-contrast fallback.
   - Reads/writes `%LOCALAPPDATA%\WinSpaces\settings.json` via `winspaces_common::Config` (single source of truth — schema, defaults, and normalization live only in `crates/winspaces-common`) and posts Win32 IPC reload messages.
   - Manages the HKCU `Run` autostart entry for the daemon (`settings_ui/autostart.rs`).

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
- [`docs/display-topology.md`](docs/display-topology.md): Stable monitor identity (`QueryDisplayConfig` device paths vs `\\.\DISPLAYn` slots), RDP topology teardown, the debounced reconcile, and per-topology layout shadow/restore.
- [`docs/ipc-and-config.md`](docs/ipc-and-config.md): Win32 IPC protocol (message window, `WM_USER` messages, UIPI filter), CLI flags, the `settings.json` and `layouts.json` schemas + normalization contract, and the elevation posture.
- [`docs/distribution.md`](docs/distribution.md): Inno Setup installer, code signing, and the release/update flow (`dev dist`, `.github/workflows/release.yml`).

## Runtime Artifacts

On launch WinSpaces reads/writes (portable mode wins if `settings.json` exists next to the `.exe`):
- Config: `%LOCALAPPDATA%\WinSpaces\settings.json`
- Log: `%LOCALAPPDATA%\WinSpaces\winspaces.log` (written via `Logger::log` in `logger.rs`)

`scripts/recover-windows.ps1` is a recovery tool: if a buggy build leaves windows cloaked/hidden after exit, run it to uncloak every top-level window and re-show the ones WinSpaces was tracking. Safe to re-run.
