# WinSpaces 🪟🦀

**WinSpaces** is an ultra-lightweight, 100% memory-safe per-monitor virtual desktop manager for Windows written in **Rust** with a native **WinUI 3** Windows 11 Settings configurator.

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
- 🎨 **Native WinUI 3 GUI Configurator:** Windows 11 Settings interface with Mica backdrop, hotkey recorder, and real-time IPC reload.
- 🌙 **Windows 11 Dark Tray Context Menu:** Native dark context menu with direct per-monitor space switching submenus.
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
- [.NET 8 SDK](https://dotnet.microsoft.com/download/dotnet/8.0)

### Compilation via Dev Task Runner
```powershell
.\dev build             # Builds Rust daemon + C# WinUI 3 GUI
.\dev run               # Runs daemon as Admin
.\dev run gui           # Launches native WinUI 3 Settings GUI
.\dev check             # Runs format, clippy, and unit tests
---

## 📚 Technical Documentation

Detailed deep-dives and engineering references:
- [`docs/dwm.md`](file:///D:/Projects/winspaces/docs/dwm.md): DWM margins, flush window snapping formulas, AUMID window fingerprinting.
- [`docs/task-view-interception.md`](file:///D:/Projects/winspaces/docs/task-view-interception.md): Mission Control architecture, DWM hardware thumbnails, and shortcut interception.

---

## 📄 License

**Proprietary — All rights reserved.** WinSpaces is closed-source software distributed commercially as a paid application. No license is granted to copy, modify, or redistribute the software.
