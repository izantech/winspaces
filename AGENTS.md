# AGENTS.md

Guidance for AI agents working in this repository.

## Project

WinSpaces is a per-monitor independent virtual desktop manager for Windows. Unlike Windows' built-in virtual desktops (which move all monitors together), each display gets its own independent set of spaces — like macOS "Displays have separate Spaces".

**Windows-only.** Uses raw Win32 FFI (`windows-sys`) in Rust for the background daemon and native .NET 8 WinUI 3 Fluent UI for the Windows 11 Settings configurator.

## Architecture & Technology Stack

The project is decoupled into two clean boundaries:

1. **Rust Daemon (`crates/winspaces-daemon`, `winspaces.exe`)**:
   - Ultra-fast, size-optimized background process (< 3 MB RAM, ~200 KB binary).
   - Manages desktop window membership, DWM cloaking, 32-bit ARGB Fluent tray icon, Windows 11 Dark context menu, and global hotkeys.
   - **Mission Control (`mission_control.rs`)**: Native GPU-accelerated Exposé overlay with live DWM thumbnails (`DwmRegisterThumbnail`), native aspect-ratio preservation (`DwmQueryThumbnailSourceSize`), top Spaces bar, and drag-and-drop window relocation across spaces.
   - **Interception & Triggers**: Single left-click on Tray icon toggles Mission Control; `WH_KEYBOARD_LL` hook intercepts `Win+Tab`; CLI switch `winspaces.exe --mission-control` sends `WM_WINSPACES_TOGGLE_MISSION_CONTROL` IPC; CLI flag `winspaces.exe --exit` / `--kill` gracefully stops the running background daemon.
   - **Taskbar & App Activation (`main.rs`)**: `EVENT_SYSTEM_FOREGROUND` and `ShellHook` (`HSHELL_WINDOWACTIVATED` / `HSHELL_RUDEAPPACTIVATED`) intercept taskbar clicks and app activations, automatically switching the target display to that window's desktop space.
   - **Window Lifecycle (`desktop.rs`)**: Automatic desktop window scanning on startup and Mission Control open; filters out Windows background services (`Windows Input Experience`, `TextInputHost`, system-cloaked windows). On startup and clean exit the daemon reclaims windows still carrying WinSpaces `SetProp` state (crash recovery), and `WM_DISPLAYCHANGE` re-maps per-monitor space state by display device name on monitor hotplug.
   - **Workspaces (`workspaces.rs`)**: Multi-monitor window layout capture and automatic rule-based placement on startup.
   - Listens for IPC reload (`WM_USER + 100`), capture (`WM_USER + 101`), restore (`WM_USER + 102`), and Mission Control (`WM_USER + 103`) messages.

2. **C# Native GUI (`gui/WinSpaces.Gui`, `WinSpaces.Gui.exe`)**:
   - Native Windows 11 Settings configurator built with .NET 8 and WinUI 3 (Windows App SDK).
   - Pure C# Fluent layout with dark theme cards, shortcut recorder, and DWM Mica material.
   - Reads/writes `%LOCALAPPDATA%\WinSpaces\settings.json` and posts Win32 IPC reload messages.
   - Manages the HKCU `Run` autostart entry for the daemon (`Services/AutostartService.cs`).
   - `ConfigModel` defaults **must mirror** `Config::default()` in `crates/winspaces-common` — both sides also normalize the desktop-hotkey lists to exactly 4 entries before use.

## Build & Run

A `dev` task runner (`dev.ps1` + `dev.cmd` shim) wraps all build and execution tasks:

```powershell
.\dev build             # Builds Rust daemon (cargo) + C# WinUI 3 GUI (dotnet)
.\dev run               # Launches Rust daemon as Admin asynchronously
.\dev run gui           # Launches native C# Windows 11 WinUI 3 GUI configurator
.\dev check             # Runs fmt + clippy + test checks
```

## Documentation

Architecture specifications and technical references (in `kebab-case`):
- [`docs/dwm.md`](file:///D:/Projects/winspaces/docs/dwm.md): DWM margins, snapping mathematics, AUMID identification, and window placement.
- [`docs/task-view-interception.md`](file:///D:/Projects/winspaces/docs/task-view-interception.md): Mission Control architecture, system shortcut interception, and window filtering.

## Runtime Artifacts

On launch WinSpaces reads/writes (portable mode wins if `settings.json` exists next to the `.exe`):
- Config: `%LOCALAPPDATA%\WinSpaces\settings.json`
- Log: `%LOCALAPPDATA%\WinSpaces\winspaces.log` (written via `Logger::log` in `logger.rs`)

`scripts/recover-windows.ps1` is a recovery tool: if a buggy build leaves windows cloaked/hidden after exit, run it to uncloak every top-level window and re-show the ones WinSpaces was tracking. Safe to re-run.
