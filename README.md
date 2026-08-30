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
- ⚡ **Minimal Footprint:** Background daemon runs at < 3 MB RAM with a ~650 KB single binary.
- 🌍 **Localized:** English and Spanish, following the Windows display language by default or pinned from Settings; translations are one JSON file each, checked at build time.
- 📑 **Modern JSON Settings:** Configured via human-readable `%LOCALAPPDATA%\WinSpaces\settings.json` (supports portable mode).
- 📝 **Real-Time Logging:** Event tracing and diagnostic logging written to `%LOCALAPPDATA%\WinSpaces\winspaces.log`.
- 🛠️ **Recovery Tool:** Includes `scripts/recover-windows.ps1` (`dev recover`) to instantly uncloak and restore windows if needed — it stops the daemon first, so recovery can't leave it half-tracking.

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
- **Right-Click**: Opens the Windows 11 Dark Context Menu (Settings live under "Configure Settings...").

---

## 🛠️ Building from Source

### Prerequisites
- [Rust Toolchain](https://www.rust-lang.org/tools/install) (`rustc` & `cargo` 1.75+)

### Compilation via Dev Task Runner
```powershell
.\dev build             # Builds the Rust workspace (daemon + settings window)
.\dev run               # Runs the daemon (non-elevated by default)
.\dev run --admin       # Runs the daemon elevated (prompts UAC)
.\dev run settings      # Opens the native settings window
.\dev check             # Runs format, clippy, and unit tests
```

---

## 📚 Technical Documentation

Detailed deep-dives and engineering references:
- [`docs/tiling.md`](docs/tiling.md): Dynamic dwindle tiling window manager, split math, gap handling, DWM margin compensation, mouse drag-swap/resize, and float rules.
- [`docs/dwm.md`](docs/dwm.md): DWM margins, flush window snapping formulas, AUMID window fingerprinting, and the DWM cloaking design.
- [`docs/mission-control.md`](docs/mission-control.md): Mission Control architecture, DWM hardware thumbnails, and shortcut interception.
- [`docs/tray-and-menu.md`](docs/tray-and-menu.md): Tray badge icon and the custom acrylic context menu — how it's drawn and why it's lightweight.
- [`docs/settings-ui.md`](docs/settings-ui.md): The native settings window — Mica backdrop, owner-drawn Fluent controls, and the hotkey recorder.
- [`docs/ipc-and-config.md`](docs/ipc-and-config.md): IPC protocol, CLI flags, and the `settings.json` configuration schema.


---

## 📄 License

**Proprietary — All rights reserved.** WinSpaces is closed-source software distributed commercially as a paid application. No license is granted to copy, modify, or redistribute the software.
