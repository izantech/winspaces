# WinSpaces 🪟🦀

**WinSpaces** is an ultra-lightweight, 100% memory-safe per-monitor virtual desktop manager for Windows written in **Rust**.

Unlike standard Windows virtual desktops (Task View) which force all monitors to switch together, **WinSpaces** gives each display its own independent set of virtual desktop spaces (similar to macOS *"Displays have separate Spaces"*).

---

## ✨ Features

- 🖥️ **Per-Monitor Independent Spaces:** Switch desktops on your primary display without affecting your secondary screens.
- 🦀 **Built in Modern Rust:** Engineered with `windows-sys` zero-cost Win32 bindings for maximum stability, safety, and performance.
- ⚡ **Minimal Footprint:** Compiles into a tiny ~135 KB executable with ~3 MB RAM usage.
- 📑 **Modern JSON Settings:** Configured via human-readable `%LOCALAPPDATA%\WinSpaces\settings.json` (supports portable mode when `settings.json` exists in the executable directory).
- 📝 **Real-Time Logging:** Event tracing and diagnostic logging written to `%LOCALAPPDATA%\WinSpaces\winspaces.log`.
- 🛠️ **Recovery Tool:** Includes `recover-windows.ps1` to instantly uncloak and restore windows if needed.

---

## ⌨️ Default Hotkeys

| Action | Default Shortcut |
| :--- | :--- |
| **Switch to Desktop 1..4** | `Alt` + `1..4` |
| **Move Window to Desktop 1..4 & Switch** | `Alt` + `Ctrl` + `1..4` |
| **Previous Desktop** | `Alt` + `Left` |
| **Next Desktop** | `Alt` + `Right` |
| **Move Window to Prev Desktop & Switch** | `Alt` + `Shift` + `Win` + `Left` |
| **Move Window to Next Desktop & Switch** | `Alt` + `Shift` + `Win` + `Right` |
| **Toggle Taskbar Visibility Mode** | `Alt` + `Ctrl` + `Shift` + `S` |
| **Exit WinSpaces** | `Alt` + `Ctrl` + `Shift` + `Q` |

---

## ⚙️ Configuration & GUI

Access settings anytime by right-clicking the **WinSpaces** system tray icon:
- **Configure Hotkeys...**: Opens the interactive Win32 hotkey configuration GUI.
- **Show all windows on taskbar**: Toggles between taskbar hiding mode (`SW_HIDE`) and forced-minimize mode (`SW_FORCEMINIMIZE`).

---

## 🛠️ Building from Source

### Prerequisites
- [Rust Toolchain](https://www.rust-lang.org/tools/install) (`rustc` & `cargo` 1.70+)

### Compilation
```powershell
# Build optimized release binary
cargo build --release
```

The compiled release binary will be created at `target/release/winspaces.exe`.

---

## 📄 License

**Proprietary — All rights reserved.** WinSpaces is closed-source software distributed commercially as a paid application. No license is granted to copy, modify, or redistribute the software.
