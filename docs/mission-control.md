# Task View, Mission Control & Win+Tab Interception

This document details the technical architecture, Win32 API mechanics, and implementation for the **WinSpaces Mission Control** overlay, system shortcut hijacking (`Win+Tab`), tray icon triggers, and window lifecycle management.

---

## 1. Mission Control Architecture

WinSpaces includes a custom, GPU-accelerated **macOS-style Mission Control** rendered natively by the background Rust daemon:

- **Backdrop**: Borderless, topmost popup with Windows 11 acrylic material (`DWMSBT_ACRYLIC`).
- **Flicker-free painting**: `WM_PAINT` renders the scene into a memory bitmap and blits it in one `BitBlt` (GDI's zero-alpha writes survive the blit, keeping the acrylic visible), hover transitions invalidate only the two affected card rects instead of the whole window, and window-card hover tracking freezes while a drag ghost is live so sweeping the grid doesn't repaint every card it crosses.
- **Thumbnail reuse on refresh**: `rebuild_cards` keeps DWM registrations keyed by source window across a refresh — kept thumbnails never leave composition, so drops and in-place space switches glide cards to their new rects instead of blinking them out and back in. A stale kept handle (update fails) demotes to a fresh registration; leftovers for windows no longer on the space are unregistered.
- **Live Hardware Video Thumbnails**: Uses Windows Desktop Window Manager (`DwmRegisterThumbnail` + `DwmUpdateThumbnailProperties`) to project live, zero-copy 60+ FPS window textures directly onto the Exposé grid.
- **Native Aspect-Ratio Preservation**: Automatically queries native source window geometry (`DwmQueryThumbnailSourceSize`) to fit thumbnails inside grid cells without vertical or horizontal distortion, dynamically framing them with dark acrylic cards.
- **Top Spaces Bar**: Displays interactive cards for Spaces 1 through 4 with live window counts and active space badges (`#818CF8`). Switching spaces while the overlay is open — by clicking a Space card, pressing `1`–`4`, or hitting the global switch/move hotkeys (e.g. `Alt+3`) — keeps Mission Control open: `refresh_mission_control` re-syncs the spaces bar and thumbnail grid **in place** (no hide/show, so no flash). Switching to the already-active space is a no-op. Only `Esc`, a backdrop click, or focusing a window dismisses the overlay.
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

### Low-Level Keyboard Hook Constraints

Two non-obvious rules keep the `WH_KEYBOARD_LL` interception alive (`low_level_keyboard_proc` in `main.rs`):

1. **Never do work inside the hook.** Windows enforces a system timeout on low-level hook callbacks; exceeding it gets the hook **silently uninstalled** and `Win+Tab` interception dies until restart. The hook therefore only `PostMessageW`s `WM_WINSPACES_TOGGLE_MISSION_CONTROL` to the daemon's message loop and returns — the overlay is built there.
2. **Inject a dummy key when swallowing `Win+Tab`.** Returning `1` eats the `Tab`, but the Shell then sees a `Win` press-and-release with no intervening key and opens the **Start menu** on key-up. The hook injects a no-op `keybd_event(0xFF)` down/up pair so the Shell counts a keystroke during the `Win` chord and suppresses Start.

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

### Eligibility Rules (structural — never title-based)

`is_valid_window` (`desktop.rs`) splits into a Win32 fact gatherer and a pure, unit-tested decision function (`is_eligible`). A window's manageability is decided by *what it is* — styles, ownership, class, cloak state — never by what its title says: titles are dynamic, localized, and collide with user content (a browser window titled "Windows Input Experience — Search Results" must stay manageable). The rules, modeled on the shell's own Alt-Tab eligibility (Raymond Chen, "Which windows appear in the Alt+Tab list?"), in check order:

1. **Liveness & process guards**: real window (`IsWindow`), resolvable pid, not WinSpaces' own process.
2. **Extended styles**: `WS_EX_TOOLWINDOW` is always excluded. `WS_EX_NOACTIVATE` (overlays, OSDs) is excluded unless `WS_EX_APPWINDOW` forces taskbar presence.
3. **Shell class blacklist**: stable system class names (`Progman`, `WorkerW`, `Shell_TrayWnd`, `Shell_SecondaryTrayWnd`, `Windows.UI.Core.CoreWindow`, `EdgeUiInputTopWndClass`, `XamlExplorerHost`, `PopupHost`, `Xaml_WindowedPopupClass`, `IME`, `MSCTFIME UI`, `tooltips_class32`, `SysShadow`, `ComboLBox`, `#32768`, ...). Classes are stable identifiers across locales and Windows builds — unlike titles. This blanket-covers the former title blacklist: IME hosts by class, Input Experience / Shell Experience Host by `Windows.UI.Core.CoreWindow`, Program Manager by `Progman`; Task Host / CoreMessaging / Push Notifications windows are simply never `WS_VISIBLE`.
4. **Empty title**: excluded — a cheap noise filter. Briefly-untitled windows are picked up by a later scan once titled; permanently untitled windows stay unmanaged (accepted limitation).
5. **Cloak state**: externally cloaked windows (`DWMWA_CLOAKED` non-zero — suspended UWP apps, other desktop software) are excluded; windows cloaked *by WinSpaces* remain valid via the `CLOAKED` state-prop bit.
6. **Visibility**: `WS_VISIBLE`-clear windows are excluded unless WinSpaces' own state bits say we hid them (DWM cloak, shell cloak, or forced minimize).
7. **Owner chain** (`GetAncestor(GA_ROOTOWNER)`): an owned window is eligible only if its root owner also passes rules 2/3/5/6 (no title requirement on the owner). Unlike Alt-Tab — which shows one representative per owner chain — real dialogs of eligible apps stay *individually* managed, because each must cloak with its app on space switches; popups of hidden or tool-window owners are excluded as noise. `WS_EX_APPWINDOW` on the window itself skips the owner judgment.

Deliberately rejected: `GetTitleBarInfo`/`STATE_SYSTEM_INVISIBLE` filtering (wrongly excludes borderless windows — see ExplorerPatcher #161) and komorebi-style `WS_CAPTION`+`WS_EX_WINDOWEDGE` requirements (too strict; forces an app-exceptions config).

System windows cloaked out of the way by the scanner (Input Experience, Task Host, IME hosts — matched by **exact** title to avoid hitting user windows) are tagged with a window property so they can be uncloaked again; substring matching is deliberately avoided.

### Automatic Desktop Window Scanning
On startup and whenever Mission Control opens, WinSpaces executes `scan_untracked_windows()` to index all running top-level application windows (Brave, VS Code, Terminal, etc.) and assign them to their respective monitor's active space.

### Crash Recovery & Display Changes
- **State reclamation**: Per-window state lives in `SetProp` window properties, which outlive the daemon process. On every startup (and on clean exit) the daemon enumerates windows still carrying a WinSpaces property and restores their visibility, so windows hidden by a crashed instance reappear automatically. `scripts/recover-windows.ps1` remains as a manual fallback.
- **`WM_DISPLAYCHANGE`**: On monitor hotplug or resolution changes the daemon rebuilds its monitor list, re-associating per-monitor space state by display device name (`\\.\DISPLAYn`), and un-hides windows that were tracked on a monitor that disappeared before re-scanning.
