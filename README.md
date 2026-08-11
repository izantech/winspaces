# WinSpaces 🪟🦀

**WinSpaces** is an ultra-lightweight, 100% memory-safe per-monitor virtual desktop manager for Windows written in **Rust**, including a native Windows 11 Settings-style configurator — one small binary, no runtimes.

Unlike standard Windows virtual desktops (Task View) which force all monitors to switch together, **WinSpaces** gives each display its own independent set of virtual desktop spaces (similar to macOS *"Displays have separate Spaces"*).

---

## ✨ Features

- 🖥️ **Per-Monitor Independent Spaces:** Switch desktops on your primary display without affecting your secondary screens.
- 🪟 **macOS-Style Mission Control:** Native, GPU-accelerated Exposé overlay with live 60+ FPS DWM window thumbnails, native aspect-ratio preservation, and top Spaces bar.
- 🎯 **Smart Taskbar & App Activation:** Clicking an application on the Windows Taskbar or launching a running instance automatically switches to that window's desktop space.
- 🖐️ **Drag-and-Drop Spaces Relocation:** Drag any window thumbnail onto a Space card in Mission Control to move it across desktops.
- 🚀 **`Win+Tab` Interception & Tray Trigger:** Replaces Windows Task View via low-level keyboard hook, tray icon single-click, or CLI shortcut (`winspaces.exe --mission-control`).
- 💼 **Workspaces Layout Save & Restore:** Save your multi-monitor application layouts and automatically restore them on startup.
- 🦀 **Built in Modern Rust:** Engineered with `windows-sys` zero-cost Win32 bindings for maximum stability, safety, and performance.
- 🎨 **Native Settings Window:** Hand-drawn Windows 11 Settings interface with real Mica backdrop, light/dark theming, hotkey recorder, and real-time IPC reload — opens instantly via `winspaces.exe --settings`.
- 🌙 **Fluent Acrylic Tray Context Menu:** Custom-drawn Windows 11 flyout with acrylic backdrop, rounded corners, Segoe Fluent Icons, light/dark theming that follows your theme live, and per-monitor space switching submenus (classic menu on Windows 10).
- 💎 **32-Bit ARGB Fluent Tray Icon:** Smooth alpha-blended badge displaying active space numbers per monitor.
- ⚡ **Minimal Footprint:** Background daemon runs at < 3 MB RAM with ~200 KB binary footprint.
- 📑 **Modern JSON Settings:** Configured via human-readable `%LOCALAPPDATA%\WinSpaces\settings.json` (supports portable mode).
- 📝 **Real-Time Logging:** Event tracing and diagnostic logging written to `%LOCALAPPDATA%\WinSpaces\winspaces.log`.
- 🛠️ **Recovery Tool:** Includes `scripts/recover-windows.ps1` to instantly uncloak and restore windows if needed.

---

## ⌨️ Default Hotkeys & Controls

| Action | Shortcut / Trigger |
| :--- | :--- |
| **Toggle Mission Control** | `Win` + `Tab` / `Ctrl` + `Up` / **Tray Icon Click** |
| **Switch to Desktop 1..4** | `Alt` + `1..4` (or press `1..4` in Mission Control) |
| **Move Window to Desktop 1..4 & Switch** | `Ctrl` + `Alt` + `1..4` (or drag window to Space card) |
| **Previous Desktop** | `Alt` + `Left` |
| **Next Desktop** | `Alt` + `Right` |
| **Move Window to Prev Desktop & Switch** | `Alt` + `Shift` + `Win` + `Left` |
| **Move Window to Next Desktop & Switch** | `Alt` + `Shift` + `Win` + `Right` |
| **Toggle Taskbar Visibility Mode** | `Alt` + `Ctrl` + `Shift` + `S` |
| **Exit WinSpaces** | `Alt` + `Ctrl` + `Shift` + `Q` |

---

## ⚙️ Configuration & Tray Controls

Access controls anytime using the **WinSpaces** system tray icon:
- **Left-Click**: Instantly toggles **Mission Control**.
- **Right-Click**: Opens the Windows 11 Dark Context Menu (Settings live under "Configure Settings...").

---

## 🛠️ Building from Source

### Prerequisites
- [Rust Toolchain](https://www.rust-lang.org/tools/install) (`rustc` & `cargo` 1.75+)

### Compilation via Dev Task Runner
```powershell
.\dev build             # Builds the Rust workspace (daemon + settings window)
.\dev run               # Runs the daemon (non-elevated)
.\dev run settings      # Opens the native settings window
.\dev check             # Runs format, clippy, and unit tests
```

---

## 📚 Technical Documentation

Detailed deep-dives and engineering references:
- [`docs/dwm.md`](docs/dwm.md): DWM margins, flush window snapping formulas, AUMID window fingerprinting, and the DWM cloaking design.
- [`docs/mission-control.md`](docs/mission-control.md): Mission Control architecture, DWM hardware thumbnails, and shortcut interception.
- [`docs/tray-and-menu.md`](docs/tray-and-menu.md): Tray badge icon and the custom acrylic context menu — how it's drawn and why it's lightweight.
- [`docs/settings-ui.md`](docs/settings-ui.md): The native settings window — Mica backdrop, owner-drawn Fluent controls, and the hotkey recorder.
- [`docs/ipc-and-config.md`](docs/ipc-and-config.md): IPC protocol, CLI flags, and the `settings.json` configuration schema.

---

## 📄 License

**Proprietary — All rights reserved.** WinSpaces is closed-source software distributed commercially as a paid application. No license is granted to copy, modify, or redistribute the software.
