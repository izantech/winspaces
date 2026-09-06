# WinSpaces 🪟🦀

**WinSpaces** is an ultra-lightweight, 100% memory-safe per-monitor spaces manager for Windows written in **Rust**, including a native Windows 11 Settings-style configurator — one small binary, no runtimes.

Unlike standard Windows virtual desktops (Task View) which force all monitors to switch together, **WinSpaces** gives each display its own independent set of spaces (similar to macOS *"Displays have separate Spaces"*).

---

## ✨ Features

- 🖥️ **Per-Monitor Independent Spaces:** Switch spaces on your primary display without affecting your secondary screens. Each display manages its own dynamic set of spaces (1–9): add more from Mission Control's "+" tile or the tray menu, remove them from the × on a space card — windows migrate to the neighboring space, macOS-style.
- 🪟 **macOS-Style Mission Control:** Native, GPU-accelerated Exposé overlay with live 60+ FPS DWM window thumbnails, native aspect-ratio preservation, and top Spaces bar.
- 🎯 **Smart Taskbar & App Activation:** Clicking an application on the Windows Taskbar or launching a running instance automatically switches to that window's space.
- 🖐️ **Drag-and-Drop Spaces Relocation:** Drag any window thumbnail onto a Space card in Mission Control to move it across spaces.
- 📌 **Sticky Windows:** Pin a window so it stays on screen across every space of its display — from the pin button on its Mission Control card or a hotkey. Pins persist with your saved layout.
- 🚀 **`Win+Tab` Interception & Tray Trigger:** Replaces Windows Task View via low-level keyboard hook, tray icon single-click, or CLI shortcut (`winspaces.exe --mission-control`).
- 💼 **Workspaces Layout Save & Restore:** Save your multi-monitor application layouts and automatically restore them on startup.
- 🔲 **Hyprland-Like Dynamic Tiling:** Automatic BSP spiral dwindle layout engine with configurable inner/outer gaps, DWM shadow margin compensation, directional focus and swap, split ratio resizing, split orientation toggle with ghost preview, drag-and-drop tile swap, border drag-resize, and persistent float rules.
- 🦀 **Built in Modern Rust:** Engineered with `windows-sys` zero-cost Win32 bindings for maximum stability, safety, and performance.
- 🎨 **Native Settings Window:** Hand-drawn Windows 11 Settings interface with real Mica backdrop, light/dark theming, hotkey recorder, and real-time IPC reload — opens instantly via `winspaces.exe --settings`.
- 🌙 **Fluent Acrylic Tray Context Menu:** Custom-drawn Windows 11 flyout with acrylic backdrop, rounded corners, Segoe Fluent Icons, light/dark theming that follows your theme live, and per-monitor space switching submenus (classic menu on Windows 10).
- 💎 **32-Bit ARGB Fluent Tray Icon:** Smooth alpha-blended badge displaying active space numbers per monitor.
- ⚡ **Minimal Footprint:** A single small native binary; the daemon idles at a few megabytes of RAM and near-zero CPU. Measured figures, dated and stamped with the machine, live in [`docs/benchmarks.md`](docs/benchmarks.md); `dev bench` reproduces the measurements.
- 🌍 **Localized:** English and Spanish, following the Windows display language by default or pinned from Settings; translations are one JSON file each, checked at build time.
- 📑 **Modern JSON Settings:** Configured via human-readable `%LOCALAPPDATA%\WinSpaces\settings.json` (supports portable mode).
- 📝 **Real-Time Logging:** Event tracing and diagnostic logging written to `%LOCALAPPDATA%\WinSpaces\winspaces.log`.
- 🛠️ **Recovery Tool:** Includes `scripts/recover-windows.ps1` (`dev recover`) to instantly uncloak and restore windows if needed — it stops the daemon first, so recovery can't leave it half-tracking. Usage and other troubleshooting: [`docs/user-guide.md`](docs/user-guide.md).

---

## ⌨️ Default Hotkeys & Controls

| Action | Shortcut / Trigger |
| :--- | :--- |
| **Toggle Mission Control** | `Win` + `Tab` / `Ctrl` + `Up` / **Tray Icon Click** |
| **Switch to Space 1..9** | `Alt` + `1..9` (or press `1..9` in Mission Control) |
| **Move Window to Space 1..9 & Switch** | `Ctrl` + `Alt` + `1..9` (or drag window to Space card) |
| **New Space** | Mission Control "+" tile (or drop a window on it) / tray submenu |
| **Remove Space** | × on a hovered Space card in Mission Control / tray submenu |
| **Previous Space** | `Alt` + `Left` |
| **Next Space** | `Alt` + `Right` |
| **Move Window to Prev Space & Switch** | `Alt` + `Shift` + `Win` + `Left` |
| **Move Window to Next Space & Switch** | `Alt` + `Shift` + `Win` + `Right` |
| **Pin Window to Every Space (sticky)** | `Alt` + `Ctrl` + `Shift` + `P` (or the pin button / `P` on a hovered card in Mission Control) |
| **Toggle Taskbar Visibility Mode** | `Alt` + `Ctrl` + `Shift` + `S` |
| **Toggle Dynamic Tiling** | `Ctrl` + `Alt` + `Shift` + `T` |
| **Focus Left / Right / Up / Down** | `Ctrl` + `Alt` + `Shift` + `←` / `→` / `↑` / `↓` |
| **Swap Left / Right / Up / Down** | `Ctrl` + `Shift` + `Win` + `←` / `→` / `↑` / `↓` |
| **Shrink / Grow Split Ratio** | `Ctrl` + `Alt` + `Shift` + `-` / `+` |
| **Toggle Float Active Window** | `Ctrl` + `Alt` + `Shift` + `F` |
| **Toggle Split Orientation** | `Ctrl` + `Alt` + `Shift` + `O` (or `Shift` + drag a tiled window) |
| **Fullscreen a Tile** | Maximize it (button, `Win` + `↑`, or drag to the top edge); restore to return it to its tile |
| **Exit WinSpaces** | `Alt` + `Ctrl` + `Shift` + `Q` |

---

## ⚙️ Configuration & Tray Controls

Access controls anytime using the **WinSpaces** system tray icon:
- **Left-Click**: Instantly toggles **Mission Control**.
- **Right-Click**: Opens the Fluent context menu (Settings, capture and restore of the workspace layout, taskbar mode, updates, exit).

## 💻 Command Line

`winspaces.exe` with no arguments starts the daemon; a second copy exits immediately. Control flags message the running daemon and return:

| Flag | Effect |
| :--- | :--- |
| `--settings` | Open the settings window (its own process) |
| `--mission-control`, `-m` | Toggle Mission Control — pin it to the taskbar as a shortcut |
| `--tiling-toggle`, `-t` | Toggle dynamic tiling |
| `--restart`, `-r` | Stop the daemon and start it again |
| `--exit`, `--kill` | Stop the daemon, restoring every hidden window first |
| `--enable-elevation`, `--disable-elevation`, `--elevation-status` | Opt-in administrator mode ([`docs/ipc-and-config.md`](docs/ipc-and-config.md) §6) |
| `--dump [file]` | Diagnostic: write every window's metrics to `window_dump.txt` (or `file`) |

---

## 🛠️ Building from Source

### Prerequisites
- [Rust Toolchain](https://www.rust-lang.org/tools/install) (`rustc` & `cargo` 1.82+; `rust-toolchain.toml` selects the channel)

### Compilation via Dev Task Runner
```powershell
.\dev build             # Builds the Rust workspace (daemon + settings window)
.\dev run               # Runs the daemon (non-elevated by default)
.\dev run --admin       # Runs the daemon elevated (prompts UAC)
.\dev run settings      # Opens the native settings window
.\dev check             # fmt, clippy, tests, per-crate checks and the windows-sys feature audit
.\dev dist              # Builds the installer into dist\
```

Linking needs the MSVC x64 desktop toolset and a Windows 10/11 SDK (Visual Studio's "Desktop development with C++" workload); see [`docs/distribution.md`](docs/distribution.md) §5.

---

## 📚 Documentation

- [`docs/user-guide.md`](docs/user-guide.md): installing, everyday use, settings and files, troubleshooting.
- [`docs/README.md`](docs/README.md): the index of the architecture pages (crate layout, IPC and configuration, DWM cloaking, Mission Control, tiling, display topology, the UI surfaces, i18n, benchmarks, distribution), with a suggested reading order.
- [`CHANGELOG.md`](CHANGELOG.md): what changed in each version.

---

## 📄 License

**Proprietary — All rights reserved.** WinSpaces is closed-source software distributed commercially as a paid application. No license is granted to copy, modify, or redistribute the software; the full terms are in [`LICENSE`](LICENSE).
