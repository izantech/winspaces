# Tray Icon & Acrylic Context Menu

This document details the technical architecture of the **WinSpaces tray surface**: the runtime-generated Fluent badge icon and the custom-drawn Windows 11 acrylic context menu, both `winspaces-ui` modules — including the Win32 mechanics that make the menu look native and the design constraints that keep both effectively free at runtime.

*Last verified: 2026-09-12, against b18bf56.*

---

## 1. Tray Icon

The notification-area icon is hand-rolled `Shell_NotifyIconW` (`NIM_ADD` / `NIM_MODIFY` / `NIM_DELETE`) — no tray crate. The icon bitmap itself is **generated at runtime** by `create_fluent_badge_icon`:

1. A 32bpp top-down DIB (`CreateDIBSection`) is filled with GDI: dark slate background (`#202020`), accent bar (`#0078D4`), subtle border, and the active space number(s) drawn in bold Segoe UI Variable.
2. A manual per-pixel alpha pass then shapes the squircle: alpha 255 inside a rounded-rect mask, 0 outside. GDI ignores the alpha channel, so this pass is what turns a square GDI painting into an anti-aliased rounded badge.
3. `CreateIconIndirect` wraps the DIB into an `HICON`; the previous icon is destroyed on every update.

The badge re-renders on every space switch (`update_state_tray_icon`), showing one number per monitor (`1|3` style). Cost: one small DIB + a few GDI fills per switch, all freed immediately.

**Interaction contract** (settled — do not revisit): a single **left-click** toggles Overview instantly; there is no double-click action and no deferred timer. **Right-click** (`WM_RBUTTONUP` / `WM_CONTEXTMENU` via the `WM_TRAYICON` callback) opens the context menu.

---

## 2. Context Menu Architecture

On Windows 11 (build ≥ 22000) the context menu is **not** an `HMENU`. It is a hand-drawn `WS_POPUP` window styled like a native Windows 11 flyout — the same approach PowerToys uses for its overlay surfaces, and the only way to get acrylic, custom row metrics, and per-item Fluent glyphs without a UI framework. Older builds fall back to a classic `TrackPopupMenu` menu, following the OS apps mode via uxtheme ordinal 135 `SetPreferredAppMode(AllowDark)`; both frontends render the same `Vec<MenuEntry>` built by the bin, so they share every command ID and handler.

### Window recipe

| Aspect | Mechanism |
| :--- | :--- |
| Window | `WS_POPUP` + `WS_EX_TOOLWINDOW \| WS_EX_TOPMOST \| WS_EX_NOACTIVATE`, owned by the hidden message window |
| Rounded corners + shadow | `DwmSetWindowAttribute(DWMWA_WINDOW_CORNER_PREFERENCE = DWMWCP_ROUND)` — DWM rounds the popup and supplies the shadow |
| Acrylic (build ≥ 22621) | `DwmExtendFrameIntoClientArea(MARGINS{-1,-1,-1,-1})` **then** `DWMWA_SYSTEMBACKDROP_TYPE = DWMSBT_TRANSIENTWINDOW` |
| Light/dark | `DWMWA_USE_IMMERSIVE_DARK_MODE` per resolved theme (also steers the acrylic tint). The palette is resolved on **every open** from `winspaces-ui`'s crate-level `theme` module — the same module the settings window reads, so the two surfaces cannot drift: App Theme selector at `HKCU\Software\WinSpaces\GuiTheme`, falling back to the OS apps mode — so the flyout tracks live theme changes with no hooks or resident state |
| 21H2 (22000–22620) | Same window, opaque background fill (no backdrop attribute available) |

Two ordering rules are load-bearing:

1. **The frame extension must precede the backdrop attribute.** Without the `-1` "sheet of glass" margins, `DWMWA_SYSTEMBACKDROP_TYPE` silently does nothing on a popup.
2. **`WS_EX_LAYERED` must never be added.** Layered windows and DWM system backdrops are mutually exclusive on one HWND; adding it would silently kill the acrylic. The space indicator is the one surface that *is* layered — it needs a real fade, which no backdrop window can do — and it pays for that by having no backdrop at all; see [`space-indicator.md`](space-indicator.md) §2.

`WS_EX_NOACTIVATE` keeps the menu from ever taking activation, so opening it never deactivates the window the user was working in — but it also means the menu **never receives keyboard focus**, which drives the input design below.

### Rendering: alpha-managed GDI

Everything is plain GDI — no Direct2D, no GDI+. The trick is manual alpha management in a 32bpp premultiplied DIB, and it now has exactly **one** implementation — `winspaces_win32::gdi::surface::paint_surface`, plus its update-rect-scoped sibling `paint_surface_clipped` — shared by the menu and the settings window rather than two hand-maintained copies:

1. The DIB is filled *directly* (slice write, not GDI) with the premultiplied background tint — `#2C2C2C` dark / `#F9F9F9` light, at alpha 232 for the menu (the settings window uses its own, slightly less opaque tint; see [`settings-ui.md`](settings-ui.md)) — so the acrylic blur shows through it.
2. Foreground is drawn with GDI: hover `RoundRect`, separator fills, `DrawTextW` labels, and Segoe Fluent Icons glyphs (checkmark `E73E`, chevron `E76C`, per-item icons). GDI **zeroes the alpha byte** of every pixel it touches.
3. A fixup pass promotes every alpha-0 pixel to opaque — text and highlights sit solid on the translucent panel.
4. `BitBlt(SRCCOPY)` copies the DIB — alpha channel included — onto the window surface, where DWM composites it over the blur.

Overview shares the crate but not this technique: it double-buffers with an *opaque* `CreateCompatibleBitmap` (`winspaces_win32::gdi::surface::double_buffer`) that never touches alpha, because it deliberately needs GDI's alpha=0 output to reach the acrylic backdrop untouched. The kit ships both as separate functions on purpose — see [`overview.md`](overview.md).

The buffer exists only inside a single `WM_PAINT`, sized to the update rect; nothing is retained between frames. Hover and keyboard-selection changes invalidate only the affected rows, so a typical repaint builds a two-row DIB rather than a whole-window one. Repaint cost figures live in [`benchmarks.md`](benchmarks.md) §5.

Layout metrics are defined at 96 dpi and scaled by the target monitor's DPI (`GetDpiForMonitor(MDT_EFFECTIVE_DPI)` — queried *before* window creation, because `GetDpiForWindow` on an unpositioned window reports the wrong monitor). Fonts are created per open and destroyed on close.

---

## 3. Input, Dismissal & the Hooks

A non-activating popup cannot use the normal focus-based input model, and `SetCapture` is only honored for the foreground thread — which a background daemon usually is not. Three mechanisms replace it, all scoped to the menu's lifetime. Light dismiss stays menu-specific by design: the settings window is a focused top-level window and closes its combo popups on ordinary deactivation, and Overview is a foreground full-screen overlay that tests backdrop clicks inline — three genuinely different surfaces with genuinely different correct answers, not one mechanism split three ways.

| Signal | Mechanism | Notes |
| :--- | :--- | :--- |
| Keyboard | The daemon's **existing** `WH_KEYBOARD_LL` hook (`low_level_keyboard_proc`, bin `handlers::shell`) | While the menu is open, nav keys (`Esc`/arrows/`Enter`/`Space`) are re-posted to the menu window and swallowed; `Win`/`Alt` close the menu (swallowing the keydown also stops the Start menu from opening on release). Adds one boolean check per keystroke when the menu is closed. |
| Outside click | A `WH_MOUSE_LL` hook installed **only while the menu is open** | Any button-down outside the menu windows posts `WM_MENU_CLOSE` and swallows the click (native menus swallow the dismissing click too). The callback is bounded — teardown always runs on the message loop, never inside the hook, respecting the LL-hook timeout rule. |
| Focus loss | Owner `WM_ACTIVATE(WA_INACTIVE)` forwarding (bin `wndproc`) | `SetForegroundWindow(owner)` runs before showing the menu; Alt-Tab or clicking another app deactivates the owner, which light-dismisses the menu. |

`SetCapture` is still taken as a best-effort extra, with one required companion: **`SetCursor(IDC_ARROW)` immediately after.** While capture is held Windows stops sending `WM_SETCURSOR`, freezing whatever cursor was active at click time — right after daemon startup that is the app-starting spinner, which then never goes away. (Overview hit the sibling of this bug via a null class cursor.)

Hover tracking needs none of this: `WM_MOUSEMOVE` is delivered to the window under the cursor regardless of activation. All hit-testing is done in **screen coordinates** against both menu windows, so it works identically whether capture engaged or not.

### Keyboard navigation

Up/Down move the selection (wrapping, skipping headers/separators), Right/Enter open a submenu or execute, Left closes a submenu, Esc closes one level at a time. Selection and hover render identically; mouse movement clears keyboard selection.

---

## 4. Submenus & Placement

- Each submenu is a **second window of the same class**, display-only; the root's handlers drive its hover state. Content is one level deep by design.
- Hover open/close uses the **system menu delay** (`SPI_GETMENUSHOWDELAY`) via `SetTimer` on the root window — not a magic number.
- Horizontal placement overlaps the root edge by 4 px like native menus and **flips to the left** when the work area runs out; vertical placement aligns the submenu's first row with the parent row (corner radius compensated).
- The root menu clamps to `MonitorFromPoint` → `rcWork` with flip-then-clamp ordering. The final clamp matters: a tray click anchors **inside the taskbar**, so the flipped bottom edge must be pulled back above `work.bottom` — without it the menu underlaps the taskbar.

Selection posts the item's command as a plain `WM_COMMAND` to the message window, so the menu shares every handler with the legacy `HMENU` path — the two menu implementations are alternative frontends over identical command IDs.

---

## 5. Why It Stays Lightweight

Current measurements — resting cost, the per-repaint figure, and the handle
deltas across open/close — live in [`benchmarks.md`](benchmarks.md) §5, along
with the recipe for reproducing them. They are deliberately **not** repeated
here: this table drifted ~8× on idle CPU before anyone noticed, precisely
because a number sitting in prose next to the design it describes has nothing
forcing it to stay true.

The shape of the result, which is what this section is actually about: the
menu returns every GDI and USER object it takes, its cost while closed is one
registered window class and one boolean check in the keyboard hook, and a full
repaint stays close to a millisecond.

The budget holds because of what the menu *doesn't* do:

1. **No UI framework, no GPU device.** Direct2D/DirectWrite would pull a D3D11 device + driver DLLs into the process (tens of MB, GPU wake-ups — EarTrumpet shipped a software-rendering fix for exactly this). GDI + ClearType is fully sufficient at menu sizes.
2. **The expensive pixels are DWM's.** Acrylic blur, corner rounding, and the shadow are composited by `dwm.exe` — the same cost as any native flyout, and none of it lands in the daemon.
3. **Everything is scoped to the open menu.** The mouse hook, timers, fonts, and windows exist only between open and close; the paint DIB exists only inside `WM_PAINT`. While closed, the feature costs one registered window class and one boolean check in the keyboard hook.
4. **No polling.** The daemon's message loop stays fully event-driven in every state; the menu adds no timers outside the transient submenu-delay timer.
5. **Zero new dependencies.** The menu uses only `windows-sys` features `winspaces-ui` already links for its other surfaces (including `Win32_UI_Controls` for the `MARGINS` struct).

The same recipe scales up to a full top-level window: the settings configurator ([`settings-ui.md`](settings-ui.md)) is the Mica variant of this technique.

## See also

- [`settings-ui.md`](settings-ui.md) for the same recipe scaled up to a window.
- [`benchmarks.md`](benchmarks.md) for the measured cost of the menu.
- [`overview.md`](overview.md) for what a tray click opens.
