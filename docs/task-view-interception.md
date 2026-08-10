# Task View, Mission Control & Win+Tab Interception

This document details the technical architecture, Win32 API mechanics, and implementation for the **WinSpaces Mission Control** overlay, system shortcut hijacking (`Win+Tab`), tray icon triggers, and window lifecycle management.

---

## 1. Mission Control Architecture

WinSpaces includes a custom, GPU-accelerated **macOS-style Mission Control** rendered natively by the background Rust daemon:

- **Backdrop**: Borderless, topmost popup with Windows 11 acrylic material (`DWMSBT_ACRYLIC`).
- **Live Hardware Video Thumbnails**: Uses Windows Desktop Window Manager (`DwmRegisterThumbnail` + `DwmUpdateThumbnailProperties`) to project live, zero-copy 60+ FPS window textures directly onto the Exposé grid.
- **Native Aspect-Ratio Preservation**: Automatically queries native source window geometry (`DwmQueryThumbnailSourceSize`) to fit thumbnails inside grid cells without vertical or horizontal distortion, dynamically framing them with dark acrylic cards.
- **Top Spaces Bar**: Displays interactive cards for Spaces 1 through 4 with live window counts, active space badges (`#818CF8`), and click-to-switch navigation.
- **Drag-and-Drop Relocation**: Users can click and drag any window thumbnail card and drop it onto a Space card in the top bar to relocate it to that desktop space.

---

## 2. Invocation Triggers

Mission Control can be toggled through four distinct triggers:

| Trigger | Mechanism | Details |
| :--- | :--- | :--- |
| **Tray Icon Left-Click** | `WM_TRAYICON (WM_LBUTTONUP)` | Clicking the tray icon immediately toggles Mission Control (<5ms). Right-click opens the context menu (Settings via "Configure Settings..."). Double-click has no separate action — each click is an instant toggle. |
| **`Win + Tab`** | `WH_KEYBOARD_LL` | Low-level keyboard hook catches `Win+Tab` before Windows Shell/DWM and opens WinSpaces Mission Control. |
| **`Ctrl + Up`** | Win32 Hotkey / Keyboard Hook | macOS-native Mission Control hotkey. |
| **`Ctrl + Mouse Button 4/5`**| AutoHotkey / Shortcut Integration | Side mouse buttons toggle Mission Control. |
| **CLI / IPC Shortcut** | `winspaces.exe --mission-control` | Posts `WM_WINSPACES_TOGGLE_MISSION_CONTROL` (`WM_USER + 103`) to the running daemon. Allows pinning a dedicated Mission Control shortcut to the Windows taskbar. |

---

## 3. Windows 11 Task View Button & Taskbar Behavior

### Taskbar Architecture (Windows 11 24H2 / Build 26100+)
In modern Windows 11, the taskbar (`Shell_TrayWnd`) is a XAML Island rendered inside `explorer.exe` using DirectComposition. Clicking the built-in Task View button is handled entirely within Explorer's internal UI event loop and does not emit standard Win32 window focus or activation events.

### Recommended Configuration
1. Open **Windows Settings → Personalization → Taskbar**.
2. Toggle the built-in **"Task view"** button to **Off**.
3. Use the **WinSpaces Tray Icon** or **`Win + Tab`** as your primary Mission Control trigger.
4. (Optional) Create a shortcut to `winspaces.exe --mission-control` and pin it to your taskbar next to the Start button.

---

## 4. Automatic Space Switching on Taskbar & App Activation

When an application window is clicked on the Windows Taskbar, opened from Start/launcher, or activated externally, WinSpaces automatically switches to the space where that window resides:

1. **Dual Interception Hook**:
   - `WinEventHook` (`EVENT_SYSTEM_FOREGROUND`): Catches all system foreground changes.
   - `ShellHook` (`HSHELL_WINDOWACTIVATED` & `HSHELL_RUDEAPPACTIVATED`): Catches direct Taskbar button and Jump List clicks.
2. **Root Owner Resolution**: Uses `GetAncestor(hwnd, GA_ROOTOWNER)` to ensure dialogs, modal sheets, and child popups resolve to their parent application window.
3. **Target Space Switching**:
   - Queries `find_window(target_hwnd)` across all displays and spaces.
   - If the window lives on an inactive space on that display, `switch_desktop(mon_idx, target_desk, Some(target_hwnd))` transitions that monitor to the correct space, uncloaks the window, and updates the Fluent tray icon badge.
   - The other monitors remain untouched (preserving per-monitor space independence).
4. **Anti-Reentrancy Suppression**: Uses `suppress_foreground_until` timestamps and flags to prevent recursive focus feedback loops during programmatic space transitions.

---

## 5. Window Filtering & Lifecycle Management

To prevent background system services from polluting Mission Control and Spaces, WinSpaces enforces strict window validation:

### Excluded Windows & Classes
- **Windows Input Experience** (`TextInputHost.exe` / `WindowsInternal.ComposableShell.Experiences.TextInput.InputApp.exe`): Virtual keyboard, Emoji picker, voice typing, and clipboard history.
- **Shell Hosts & Workers**: `Progman`, `WorkerW`, `Shell_TrayWnd`, `Shell_SecondaryTrayWnd`, `XamlExplorerHost`, `TopLevelWindowForOverflowXamlIsland`.
- **Popup & Tool Windows**: `PopupHost`, `Popup`, `SysShadow`, `tooltips_class32`, `ComboLBox`, `#32768`, and any window with `WS_EX_TOOLWINDOW`.
- **UWP Core Windows**: `Windows.UI.Core.CoreWindow`, `EdgeUiInputTopWndClass`.
- **Cloaked System Windows**: Windows where `DwmGetWindowAttribute(hwnd, DWMWA_CLOAKED, ...)` is non-zero (unless cloaked directly by WinSpaces).

System windows cloaked out of the way by the scanner (Input Experience, Task Host, IME hosts — matched by **exact** title to avoid hitting user windows) are tagged with a window property so they can be uncloaked again; substring matching is deliberately avoided.

### Automatic Desktop Window Scanning
On startup and whenever Mission Control opens, WinSpaces executes `scan_untracked_windows()` to index all running top-level application windows (Brave, VS Code, Terminal, etc.) and assign them to their respective monitor's active space.

### Crash Recovery & Display Changes
- **State reclamation**: Per-window state lives in `SetProp` window properties, which outlive the daemon process. On every startup (and on clean exit) the daemon enumerates windows still carrying a WinSpaces property and restores their visibility, so windows hidden by a crashed instance reappear automatically. `scripts/recover-windows.ps1` remains as a manual fallback.
- **`WM_DISPLAYCHANGE`**: On monitor hotplug or resolution changes the daemon rebuilds its monitor list, re-associating per-monitor space state by display device name (`\\.\DISPLAYn`), and un-hides windows that were tracked on a monitor that disappeared before re-scanning.
