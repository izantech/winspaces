# Task View, Mission Control & Win+Tab Interception

This document details the technical architecture, Win32 API mechanics, and implementation for the **WinSpaces Mission Control** overlay, system shortcut hijacking (`Win+Tab`), tray icon triggers, and window lifecycle management.

---

## 1. Mission Control Architecture

WinSpaces includes a custom, GPU-accelerated **macOS-style Mission Control** rendered natively by the background Rust daemon:

- **Backdrop**: Borderless, topmost popup with Windows 11 acrylic material (`DWMSBT_ACRYLIC`).
- **Flicker-free painting**: `WM_PAINT` renders the scene into a memory bitmap and blits it in one `BitBlt` (GDI's zero-alpha writes survive the blit, keeping the acrylic visible), hover transitions invalidate only the two affected card rects instead of the whole window, and window-card hover tracking freezes while a drag ghost is live so sweeping the grid doesn't repaint every card it crosses. The back buffer covers `ps.rcPaint`, not the client area — the overlay is monitor-sized, so a full-screen bitmap per paint would put a 4K allocation behind every hover tint and every drag frame. Shifting the memory DC's viewport origin lets the renderer keep drawing in absolute client coordinates while GDI clips the rest.
- **Thumbnail reuse on refresh**: `rebuild_cards` keeps DWM registrations keyed by source window across a refresh — kept thumbnails never leave composition, so drops and in-place space switches glide cards to their new rects instead of blinking them out and back in. A stale kept handle (update fails) demotes to a fresh registration; leftovers for windows no longer on the space are unregistered.
- **Live Hardware Video Thumbnails**: Uses Windows Desktop Window Manager (`DwmRegisterThumbnail` + `DwmUpdateThumbnailProperties`) to project live, zero-copy 60+ FPS window textures directly onto the Exposé grid.
- **Native Aspect-Ratio Preservation**: Automatically queries native source window geometry (`DwmQueryThumbnailSourceSize`) to fit thumbnails inside grid cells without vertical or horizontal distortion, dynamically framing them with dark acrylic cards.
- **Top Spaces Bar**: Displays one interactive card per space on the active monitor (each monitor has its own dynamic count, 1–9) with live window counts and active space badges (`#818CF8`). Switching spaces while the overlay is open — by clicking a Space card, pressing `1`–`9`, or hitting the global switch/move hotkeys (e.g. `Alt+3`) — keeps Mission Control open: `refresh_mission_control` re-syncs the spaces bar and thumbnail grid **in place** (no hide/show, so no flash). Switching to the already-active space is a no-op. Only `Esc`, a backdrop click, or focusing a window dismisses the overlay. When the natural strip would overflow a narrow monitor, `spaces_bar_metrics` shrinks card widths toward a floor instead of letting the strip run off-screen.
- **Reordering Spaces (macOS-style)**: Users can click and drag any Space card horizontally across the top bar to reorder spaces on that display, or press `Ctrl+Shift+←/→` to walk the active space one slot (a no-op at either end). Only horizontal travel past `SM_CXDRAG` lifts a card — reordering is a horizontal gesture, so a vertical nudge must not detach it. While dragging, sibling cards shift to illustrate the new order, a placeholder marks the vacated slot, and the dragged card floats with an accent border (`#818CF8`), pinned to the bar's `y` because DWM composites the thumbnail grid *above* this window's GDI output. It is painted after every other tile in the bar, including the "New Space" tile, so it rides over what it passes instead of sliding underneath. `Esc` cancels a live drag without dismissing the overlay. Only the pointer's x decides the drop — the card is pinned to the bar, so the cursor is free to roam anywhere over the overlay without losing the move. Dropping reorders monitor space membership in `DesktopManager::reorder_space`, preserves active space identity and window tracking, updates the Fluent tray badge, and refreshes the overlay in place without flashing. Drag frames invalidate the spaces-bar strip (`spaces_bar_strip_rect`) — always the whole strip, never the band between two card positions, because the throttle below drops frames, so the last *painted* position is not the previous message's position and a band measured from the latter leaves a trail of ghost cards behind. Paints are throttled to one blit per display frame (`frame_interval_ms` from `GetDeviceCaps(VREFRESH)`, with a one-shot `TIMER_DRAG_PAINT` for the trailing position). A mouse reports several times faster than the panel refreshes, and painting a DWM-composited window more than once per frame lets the compositor sample a half-written surface — which reads as the card tearing across a scanline.
- **Add / Remove Spaces (macOS-style)**: A "New Space" tile after the last space card appends a space (hidden at the 9-space cap). It is deliberately *not* in the space cards' visual family — a dimmer background behind a dashed border, with the same Segoe Fluent Icons "Add" glyph (`GLYPH_ADD`) over a caption, tinting indigo on hover. Label and glyph both match the tray menu's New Space item, so the action reads the same in both surfaces. In card colours with a bare "+" it read as an N+1'th space rather than an action. Hovering a space card reveals an × close button (hidden when it is the monitor's last space) that removes the space — its windows migrate to the space on the left (the first space's windows fall right) and indices shift down. The tile deliberately lives *outside* `space_cards` (a separate `plus_rect`), so every consumer of that vector can assume it contains real spaces only; the drop handler resolves targets by the card's `desk_idx`, never its vector position. Both actions funnel through the bin's `add_space_on` / `remove_space_on` choke points via the `McHost.add_space` / `McHost.remove_space` entries (§1.1), which persist the count, re-register digit hotkeys when the cross-monitor maximum changes, update the tray, and refresh the open overlay.
- **Drag-and-Drop Relocation**: Users can click and drag any window thumbnail card and drop it onto a Space card in the top bar to relocate it to that desktop space — or onto the "New Space" tile to create a new space with that window on it.

---

## 1.1 The `McHost` Indirection

Mission Control lives in `winspaces-ui` and never sees the bin's application state. Its entry points (`toggle_mission_control`, `show_mission_control`, `refresh_mission_control`) take `&mut DesktopManager` directly — enough for everything the overlay does on its own (scanning untracked windows, reading monitor/space data, rebuilding the card grid). What it *cannot* do itself — because the action must also update the persisted space count, re-register digit hotkeys, refresh the tray badge, or otherwise touch state a UI crate must not depend on — goes through `McHost`, a plain struct of seven `fn` pointers (`add_space`, `remove_space`, `reorder_space`, `reorder_space_neighbor`, `switch_space`, `move_window_to_space`, `move_window_to_new_space`). The bin builds one static `McHost` whose functions each wrap the exact `with_app_state` block that used to sit inline in this module, and installs it once at startup (`install_host`); Mission Control reaches it through a private `OnceLock`.

**Why fn pointers instead of posted messages.** The obvious alternative — have Mission Control `PostMessage` its intent to the daemon's message window — is architecturally cleaner but defers the action to the next message-loop turn. Today, `WM_LBUTTONUP` completes a space reorder *and* the resulting overlay refresh synchronously, before the handler returns. Deferring either half through `PostMessage` would change the frame in which the overlay repaints after a drop — visible as a stray flash or a stale frame — which is exactly the class of bug a test suite cannot catch. A direct `fn` call preserves the original synchronous behavior; only the *name* Mission Control calls through changed, not when the call happens or what state it borrows.

This is also why the previous shape — the bin and Mission Control calling into each other directly — was a genuine bidirectional dependency cycle, not just an inconvenience: the bin needed Mission Control's refresh function, and Mission Control needed the bin's state-mutating helpers. Splitting them into separate crates forced the inversion: the crate boundary means the bin's helpers now flow *into* Mission Control as data (function pointers) rather than Mission Control naming the bin's functions directly, so the dependency only ever points from `winspaces` down to `winspaces-ui`, never back.

**The geometry payoff.** Mission Control's hit-testing and layout math — spaces-bar metrics, card rects, drop targets — live in a `geometry` submodule that is 100% pure: no `HWND`, no Win32 call, nothing but rects and indices in, rects and booleans out. That purity is what makes it unit-testable in isolation, and the crate split is what makes the boundary enforceable rather than aspirational. One deliberate asymmetry lives there: `geometry`'s point-in-rect test is **inclusive** of the right/bottom edge, while the tray menu's and the settings window's are **exclusive**. `winspaces-win32` ships no shared `pt_in_rect` at all — not an oversight, a refusal — because unifying the two semantics would silently shift every Mission Control hit target (most visibly the card close button and card edges, which sit exactly on the boundary) by a pixel. Mission Control keeps its own, commented as deliberately different.

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

Two non-obvious rules keep the `WH_KEYBOARD_LL` interception alive (`low_level_keyboard_proc`, bin `handlers::shell`):

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

`is_valid_window` (`winspaces-core`'s `desktop` module) splits into a Win32 fact gatherer and a pure, unit-tested decision function (`is_eligible`). A window's manageability is decided by *what it is* — styles, ownership, class, cloak state — never by what its title says: titles are dynamic, localized, and collide with user content (a browser window titled "Windows Input Experience — Search Results" must stay manageable). The rules, modeled on the shell's own Alt-Tab eligibility (Raymond Chen, "Which windows appear in the Alt+Tab list?"), in check order:

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

### Untracking Closed Windows
Tracking is added by the scanner but, for a long time, was never removed: `remove_window` was only reached when a window *moved* between spaces, and `EnumWindows` cannot notice a closed window because it only yields live ones. Dead handles therefore accumulated for the daemon's lifetime. Most read sites filter with `is_valid_window` and so were unaffected, but the space card's window count read the raw list length and drifted — a card reading "10 windows" above two thumbnails.

Three layers now keep the list honest, deliberately overlapping:

1. **`HSHELL_WINDOWDESTROYED`** untracks at the moment of closing, and re-syncs an open overlay so a card cannot outlive its window. **This handler is guarded on `is_live_window` and that guard is load-bearing**: the shell also sends this event when a window merely leaves its window list, which WinSpaces' own cloaking causes on a space switch. Acting on the raw event would silently discard the user's space assignment for a window that is only hidden.
2. **`prune_dead_windows()`** runs at the top of every `scan_untracked_windows()` and sweeps anything the hook missed. It uses `is_live_window` (null + `IsWindow`) rather than the fuller `is_valid_window`, for the same reason: a window hidden on an inactive space is intentionally invisible, and a momentary eligibility failure must not delete it. Only a genuinely dead handle is dropped.
3. **The card count filters at render time**, matching the Exposé grid's own filter exactly, so the label and the thumbnails cannot disagree regardless of the tracked list's state.

### Crash Recovery & Display Changes
- **State reclamation**: Per-window state lives in `SetProp` window properties, which outlive the daemon process. On every startup (and on clean exit) the daemon enumerates windows still carrying a WinSpaces property and restores their visibility, so windows hidden by a crashed instance reappear automatically. `scripts/recover-windows.ps1` remains as a manual fallback.
- **`WM_DISPLAYCHANGE`**: On monitor hotplug or resolution changes the daemon rebuilds its monitor list, re-associating per-monitor space state by display device name (`\\.\DISPLAYn`), and un-hides windows that were tracked on a monitor that disappeared before re-scanning.
