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
- **Top Spaces Bar**: Displays one interactive card per space on the active monitor (each monitor has its own dynamic count, 1–9) with live window counts and active space badges (`#818CF8`). Switching spaces while the overlay is open — by clicking a Space card, pressing `1`–`9`, or hitting the global switch/move hotkeys (e.g. `Alt+3`) — keeps Mission Control open: `refresh_mission_control` re-syncs the spaces bar and thumbnail grid **in place** (no hide/show, so no flash). Switching to the already-active space is a no-op. Only `Esc`, a backdrop click, or focusing a window dismisses the overlay. When the natural strip would overflow a narrow monitor, `spaces_bar_metrics` shrinks card widths toward a floor instead of letting the strip run off-screen.
- **Add / Remove Spaces (macOS-style)**: A "+" tile after the last space card appends a space (hidden at the 9-space cap); hovering a space card reveals an × close button (hidden when it is the monitor's last space) that removes the space — its windows migrate to the space on the left (the first space's windows fall right) and indices shift down. The "+" tile deliberately lives *outside* `space_cards` (a separate `plus_rect`), so every consumer of that vector can assume it contains real spaces only; the drop handler resolves targets by the card's `desk_idx`, never its vector position. Both actions funnel through the `add_space_on` / `remove_space_on` choke points in `main.rs`, which persist the count, re-register digit hotkeys when the cross-monitor maximum changes, update the tray, and refresh the open overlay.
- **Drag-and-Drop Relocation**: Users can click and drag any window thumbnail card and drop it onto a Space card in the top bar to relocate it to that desktop space — or onto the "+" tile to create a new space with that window on it.

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

---

## 6. Transition Animations

Open, close, and space-switch each tween the overlay from a captured start state to the layout `rebuild_cards` already computes, driven by a `WM_TIMER` owned by the overlay HWND itself. No new crate, no DirectComposition/Direct2D/WinRT, no extra `DwmRegisterThumbnail` calls per frame, and no `DwmFlush` anywhere in the crate.

### Frame driver

- Timer id `TIMER_MC_ANIM = 10` (the overlay owns no other timer), armed with `SetTimer(mc.hwnd, TIMER_MC_ANIM, MC_ANIM_TICK_MS, None)` at `MC_ANIM_TICK_MS = 15` ms — close to the ~15.6 ms system compositor tick.
- Three durations, one per `AnimKind`: Open `MC_ANIM_OPEN_MS = 200.0`, Close `MC_ANIM_CLOSE_MS = 160.0`, Switch `MC_ANIM_SWITCH_MS = 160.0`.
- `start_animation` is the only arm point: it sets `mc.anim`, calls `SetTimer`, then immediately calls `tick_animation` to compute frame 0 in place, so the first paint after a transition starts is never a tick late.
- `complete_animation` is the only disarm point (`KillTimer`). It snaps every card's `draw_rect` to `card_rect` and its thumbnail back to the resting rect/opacity 255 via the existing `restore_thumbnail`, then — Close only — runs the deferred teardown (below).
- Progress is wall-clock, not tick-counted: each call computes `t_raw` from `Instant::elapsed()`, so a dropped tick shortens the animation instead of stretching it. `eased = ease_out_cubic(t_raw)` for Open/Switch, `ease_in_quad(t_raw)` for Close.
- The tick never calls `UpdateWindow` / `RedrawWindow(RDW_UPDATENOW)`, only `InvalidateRect`. A synchronous repaint from inside the tick would re-enter `render_mission_control`'s `MC_STATE.borrow()` while the tick still holds `borrow_mut()` — an instant panic. `WM_PAINT` and `WM_TIMER` are both queue-empty messages, so the daemon's message loop dispatches the paint on its own before the next tick fires.

### Tweened quantities

Each incoming `WindowCard` carries `anim_from_card` / `anim_from_thumb` (its animation start rects) alongside its resting `card_rect` / `thumb_rect`. Every tick:

- `card.draw_rect = lerp_rect(from, to, eased)` — every render path reads `draw_rect`, never `card_rect`, so this is the single geometry source for painting during a transition.
- One `DwmUpdateThumbnailProperties` per card sets `rcDestination` to the same lerp applied to `thumb_rect`, plus a genuine DWM opacity fade: `255 * eased` on Open, `255 * (1 - eased)` on Close, a flat `255` on Switch (Switch moves the incoming set without fading it).
- GDI has no real alpha, so the card fill/border fake it: `lerp_rgb` blends between the backdrop colour `rgb(0x14,0x14,0x18)` and the resting card colours `rgb(0x1F,0x1F,0x24)` / `rgb(0x38,0x38,0x42)` by the same fade factor (Switch's fade factor is a constant `1.0` — its cards stay fully opaque; the only real Switch fade is the thumbnail opacity on the outgoing set, below). While this lerp is active, `DrawIconEx` and the title `DrawTextW` — the two most expensive per-card GDI calls — plus all hover/drag decoration are skipped outright; they're unreadable during a 160–200 ms move anyway.
- The spaces bar gets one uniform `bar_dy` offset (`lerp(-half_card_h, 0)` on Open, reversed on Close, `0` on Switch) applied to every space card and the `+` tile at draw time — no new fields on `SpaceCard`. Switch instead crossfades the active tint: the departing card's fill/pen lerp from the active indigo back to resting colours while the arriving card lerps the other way, keyed off `Animation::prev_active`.

### Entrance vs. exit

`anim_endpoints(kind, anim_from, final_rect)` resolves each card's per-tick `(from, to)` pair: `(anim_from, final_rect)` for Open and Switch, but `(final_rect, anim_from)` for Close — the swap that turns Close into an exit instead of an entrance. Open/Switch cards start away from their resting spot and arrive there as `eased` runs 0→1. A Close card is already at `final_rect` when the animation starts — it's been sitting in the grid — and tweens back toward `anim_from`, which is why Close cannot reuse whatever `anim_from_*` happened to hold from the last `rebuild_cards` call: `hide_mission_control_focusing` recaptures every visible card's source rects live, via `capture_anim_source_rects`, immediately before building the Close `Animation`. Without that recapture, `anim_from_*` would still hold Open's real-window rects or a Switch's slide-in offset rects — stale in either case, and never the window's actual current position.

`capture_anim_source_rects` (shared by the Open path inside `rebuild_cards` and by this Close recapture) reads `DWMWA_EXTENDED_FRAME_BOUNDS` (falling back to `GetWindowRect`) and converts it to overlay client coordinates; it falls back to a self-zoom of the card's own final rect (scaled by `MC_ANIM_SELF_ZOOM = 0.85` about its centre) when the source window is minimized, degenerate, or doesn't intersect the overlay at all.

### Nesting invariant and frame/thumbnail agreement

`thumb_rect` is always a subset of `card_rect`, and `anim_from_thumb` derives `anim_from_card` (or vice versa) by the same header/margin offsets — so the component-wise lerp of two nested rect pairs stays nested at every `t`, and the card frame can never expose a gap around the thumbnail mid-tween. `tick_animation` reads and writes a single `eased` value once per tick, and `render_mission_control` reads that same stored value once at the top of the paint (`mc.anim.as_ref().map(|a| (a.kind, a.eased))`) — so the DWM thumbnail update and the GDI paint it drives always agree on the same `t`, rather than approximately. Because the card's opaque fill covers the whole card interior including the thumbnail area, any residual one-frame skew between the two reveals card colour underneath, never the acrylic backdrop through a gap.

### Cross-pushed outgoing set (Switch)

A space switch cross-pushes two sets of cards past each other. `rebuild_cards`, called with `AnimRequest::Switch { push_dx }`, snapshots the pre-switch `window_cards` before clearing them, rebuilds the incoming grid for the new space, and returns the leftover cards — those whose DWM handle was not reused by the new grid — as its `Vec<WindowCard>` return value; ownership transfers into `Animation::outgoing` in the caller. Each tick offsets the outgoing cards by `-push_dx * eased` and fades their thumbnail opacity to `255 * (1 - eased)`; they're drawn first, so the incoming set (arriving from `+push_dx`) paints on top of them. `complete_animation` unregisters their thumbnails. Outgoing cards are deliberately never inserted into `window_cards`, so hit-testing, drag state, and hover tracking need no special case for them.

A switch onto a space with zero windows still has to let that outgoing set finish sliding away, so `render_mission_control`'s "No open windows on Space N" placeholder only short-circuits when `window_cards` is empty **and** the in-flight animation's `outgoing` is also empty (the `has_outgoing` guard) — otherwise the departing cards would vanish instantly instead of animating out.

### Snap-to-final re-entrancy policy

One rule, applied uniformly: every entry point that can start or observe a transition — `show_mission_control`, `hide_mission_control_focusing`, `refresh_mission_control_animated`, and the `WM_KEYDOWN` / `WM_LBUTTONDOWN` / `WM_LBUTTONUP` handlers in `mc_wnd_proc` — calls `commit_pending` first. `commit_pending` runs `complete_animation` whenever `mc.anim.is_some()`, snapping every animated quantity to its resting state (running a pending Close's teardown if that's what was in flight) before the new request is evaluated. The overlay is a process-wide singleton (`MC_STATE` is a `thread_local`), so there is exactly one animation in flight at a time by construction. `WM_MOUSEMOVE` additionally returns early while `mc.anim.is_some()`, so hover repaints never fight the tween — clicks are never swallowed by this, because hit-testing always tests against `card_rect`, never the animated `draw_rect`.

`complete_animation` and `commit_pending` return the focus `HWND` to raise (or null) instead of calling `SetForegroundWindow` themselves; every caller does so only after releasing the `MC_STATE` borrow (`ShowWindow(SW_HIDE)` is fine inside the borrow — only `SetForegroundWindow` is not).

### Deferred close teardown

`hide_mission_control_focusing` sets `is_visible = false` and clears hover/drag state immediately — `is_mission_control_active()` stays honest and a re-toggle opens fresh — but defers the actual teardown (unregistering every thumbnail, clearing `window_cards`/`space_cards`, `ShowWindow(SW_HIDE)`) into `complete_animation`, which only runs once the Close animation's `eased` reaches 1.0 (or immediately, when animations are disabled for this transition). Deferring the teardown is what makes a close animation possible at all — the overlay has to stay live and on-screen while it shrinks back toward the real windows.

That deferral creates one risk: a daemon exit or session end arriving mid-close would otherwise leak the still-registered DWM thumbnails. `finish_animation_now()` — a thin `commit_pending` wrapper callable from outside the module — is called from the daemon's cleanup block (before `windows_show_all()`) and from the `WM_ENDSESSION` handler in `main.rs`, guaranteeing a shutdown mid-close still runs the deferred `DwmUnregisterThumbnail` + `SW_HIDE`.

### Three-way degradation

`anim_allowed(mc, card_count)` gates every transition and is true only when all three hold:

1. `mc.animations_enabled` — the `mission_control_animations` config setting (see [`ipc-and-config.md`](ipc-and-config.md)), cached on the overlay and kept live by `set_animations_enabled`, called from the `WM_WINSPACES_RELOAD_CONFIG` handler.
2. `card_count <= MC_ANIM_MAX_CARDS` (`24`) — an explicit guardrail, **not a measured number**: it sits just past the point where the grid drops to 5 columns and cards shrink below the 180 px minimum. Per-frame cost of many simultaneous `DwmUpdateThumbnailProperties` calls is unmeasured beyond small card counts. The evidence-gathering mechanism is a one-line `winspaces.log` warning whenever a transition's wall-clock time overruns its nominal duration by `MC_ANIM_OVERRUN_FACTOR` (`1.5×`), logged from `tick_animation` on completion with the kind name and both durations. If that line starts showing up in practice, `MC_ANIM_MAX_CARDS` is the first thing to revisit.
3. `system_animations_enabled()` — a fail-open `SystemParametersInfoW(SPI_GETCLIENTAREAANIMATION, ...)` probe (Settings > Accessibility > Visual effects > Animation effects). Read fresh at the start of every transition, never cached, so a live setting change takes effect on the next open/close/switch with no `WM_SETTINGCHANGE` handler needed.

When any of the three is false, the call site still builds the `Animation` exactly as it would for a real transition but skips `start_animation`: the animation is stashed straight into `mc.anim` and `complete_animation` is called on it immediately (`show_mission_control` instead just never starts one, since `rebuild_cards` already wrote final rects and thumbnail state). This keeps `DwmRegisterThumbnail` / `DwmUnregisterThumbnail` confined to `rebuild_cards` and `complete_animation` in every case — the degraded path re-runs the same completion code with zero duration and no timer armed, rather than branching into a separate DWM-registration path.

### Non-animated by design

`add_space_on` / `remove_space_on` (`main.rs`) and the drag-drop move / at-cap rebuild call sites in `mc_wnd_proc` all use the plain `refresh_mission_control` wrapper, which calls `refresh_mission_control_animated(app_state, false)`. Animation is opt-in per call site, not a default: only the digit-key switch and the space-card click pass `animate: true`, and even then `refresh_mission_control_animated` only actually animates when the displayed space changes (`prev_desk != desk_idx`) — a move-window hotkey or a drag-drop refresh that leaves the shown space unchanged stays instant either way.
