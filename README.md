<img src="assets/logo.svg" width="96" align="right" alt="">

# WinSpaces

**Independent spaces per monitor for Windows, macOS-style Mission Control, and dynamic tiling.**  
*Ultra-lightweight, native Win32, 100% memory-safe Rust — single small binary, zero runtimes.*

[![License: GPL-3.0](https://img.shields.io/badge/License-GPLv3-blue.svg)](LICENSE)
[![Release](https://img.shields.io/github/v/release/izantech/winspaces?include_prereleases&label=release)](https://github.com/izantech/winspaces/releases)
[![Platform](https://img.shields.io/badge/Platform-Windows%2010%20%7C%2011%20(x64)-informational)](https://github.com/izantech/winspaces)

Standard Windows virtual desktops switch every monitor at the same time. **WinSpaces** gives each display its own independent set of spaces (1–9), a live GPU-accelerated Mission Control overlay, and optional dynamic tiling.

---

## ✨ Highlights

- 🖥️ **Per-Monitor Independent Spaces**: Switch spaces on your primary display without affecting secondary screens. Add or remove spaces dynamically per monitor (1–9).
- 🪟 **macOS-Style Mission Control**: Hardware-accelerated Exposé overlay (`Win+Tab`) with live 60+ FPS window thumbnails, top spaces bar, and drag-and-drop window relocation.
- 🔲 **Hyprland-Like Dynamic Tiling**: Automatic BSP spiral dwindle layout with configurable gaps, border drag-resize, split orientation toggle, and persistent float rules.
- ⚡ **Minimal Footprint**: Single small native binary that idles at a few megabytes of RAM and near-zero CPU; measured figures in [`docs/benchmarks.md`](docs/benchmarks.md). Built on raw Win32 FFI (`windows-sys`).
- 🎨 **Windows 11 Native UI**: Hand-drawn Settings window with real Mica backdrop, acrylic tray menu with Segoe Fluent Icons, and active space indicators.

👉 **[winspaces.app](https://winspaces.app)** — feature showcase, hotkey cheatsheet and quick start.

---

## ⌨️ Essential Hotkeys

| Action | Shortcut / Trigger |
| :--- | :--- |
| **Toggle Mission Control** | `Win` + `Tab` / `Ctrl` + `Up` / **Tray Icon Click** |
| **Switch to Space 1..9** | `Alt` + `1..9` |
| **Move Window to Space 1..9 & Follow** | `Ctrl` + `Alt` + `1..9` |
| **Previous / Next Space** | `Alt` + `Left` / `Alt` + `Right` |
| **Pin Window to All Spaces (Sticky)** | `Ctrl` + `Alt` + `Shift` + `P` (or pin in Mission Control) |
| **Toggle Dynamic Tiling** | `Ctrl` + `Alt` + `Shift` + `T` |

*Every hotkey can be customized in the native Settings window (`winspaces.exe --settings`). Complete hotkey reference: [`docs/user-guide.md`](docs/user-guide.md#9-hotkey-reference).*

---

## 📦 Installation

- **Installer (Recommended)**: Download `WinSpaces-Setup-x64-<version>.exe` from [Releases](https://github.com/izantech/winspaces/releases/latest). Installs per-user without requiring administrator rights.
- **Portable Mode**: Place `winspaces.exe` in any folder alongside an empty `settings.json`. All configuration, layouts, and logs remain in that directory.

---

## 🛠️ Building from Source

Requires the [Rust Toolchain](https://www.rust-lang.org/tools/install) (1.82+) and the MSVC x64 desktop toolset with a Windows 10/11 SDK (Visual Studio's "Desktop development with C++" workload); see [`docs/distribution.md`](docs/distribution.md) §5.

```powershell
.\dev build             # Compile workspace (daemon + settings window)
.\dev run               # Run daemon in notification area
.\dev run settings      # Open native settings window
.\dev check             # Run fmt, clippy, tests, and windows-sys feature audit
.\dev site              # Serve the website locally (http://127.0.0.1:8338)
.\dev dist              # Build the standalone installer into dist\
```

---

## 📚 Documentation

- [📖 User Guide](docs/user-guide.md): Installation, first steps, workspaces layout persistence, tiling, and troubleshooting.
- [🏛️ Architecture & Internals](docs/README.md): Crate layout, DWM cloaking, IPC protocol, display topology, and contributor guide.
- [📊 Benchmarks](docs/benchmarks.md): Measured latency, CPU, and memory numbers reproducible via `dev bench`.
- [📝 Changelog](CHANGELOG.md): Release notes and version history.

---

## 🛡️ Support & Maintenance

WinSpaces is free software maintained in personal spare time, best-effort: no support team, no SLA, no commitment to updates or compatibility fixes for future Windows releases. Issues and pull requests are welcome; forks are fine under the GPLv3.

---

## 📄 License

Copyright (C) 2026 izantech <dev@izantech.app>.

WinSpaces is free and open-source software licensed under the [GNU General Public License v3.0](LICENSE) (GPL-3.0-or-later).
