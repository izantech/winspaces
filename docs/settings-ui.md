# Settings Window: Native Fluent UI in the Daemon Binary

How `winspaces.exe --settings` draws a Windows 11 Settings-style configurator with nothing but Win32, GDI, and DWM — the same technique as the tray menu ([`tray-and-menu.md`](tray-and-menu.md) §2), scaled up to a resizable top-level window. Code lives in `crates/winspaces-daemon/src/settings_ui/`.

---

## 1. Process Model

`--settings` is parsed in `main()` before any daemon initialization; `settings_ui::run_settings()` runs a plain message loop and returns. Consequences:

- **Separate process.** The tray "Settings" item (and the hotkey-failure fallback) spawn `current_exe()` with `--settings`. A settings crash cannot touch the daemon, and the daemon carries **zero** runtime cost for the settings code while it is closed — the module is unreachable from daemon paths and its pages are never resident.
- **Single settings instance.** `run_settings` first does `FindWindowW("WinSpacesSettingsClass", "WinSpaces Settings")`; if found it restores + foregrounds that window and exits.
- **No daemon required.** The window works standalone (the hero card shows *Daemon Stopped*); IPC posts are no-ops with a warning banner when no daemon window exists.

## 2. Window & Rendering Recipe

- `WS_OVERLAPPEDWINDOW`, 960x720 dips, min 700x500 (`WM_GETMINMAXINFO`), standard system caption themed via `DWMWA_USE_IMMERSIVE_DARK_MODE` — snap layouts and caption buttons come free (the Notepad look).
- **Real Mica**: `DwmExtendFrameIntoClientArea(-1)` then `DWMWA_SYSTEMBACKDROP_TYPE = DWMSBT_MAINWINDOW`, gated on build ≥ 22621. Pre-Mica builds and high-contrast mode fall back to an opaque themed background.
- **Painting** (`controls::paint_surface`): every `WM_PAINT` renders the whole client area into a premultiplied 32bpp top-down DIB — base tint at alpha 216 so Mica shows through, GDI foreground (ClearType text via `CreateFontW`, `RoundRect` cards), then the alpha fixup promoting every GDI-touched pixel to opaque, and a single `BitBlt`. Full-window repaint per interaction is deliberate; Mission Control repaints entire screens the same way.
- **DPI**: per-monitor v2 (set process-wide in `main`). All metrics are 96-dpi constants scaled through a `px()` closure; `WM_DPICHANGED` adopts the suggested rect, recreates fonts, and relayouts.
- **Theme**: `theme.rs` owns every color (palette tokens; no ARGB literals elsewhere). Preference (`system`/`light`/`dark`) persists at `HKCU\Software\WinSpaces\GuiTheme`; system resolution reads `AppsUseLightTheme`, accent comes from `DwmGetColorizationColor`, and `WM_SETTINGCHANGE` rebuilds the palette live. High contrast derives the palette from `GetSysColor`.

## 3. Owner-Drawn Control Kit

All controls are regions of the single window — one wndproc, one flat focus model, no child HWNDs (the sole exception below). `pages.rs` declares each page as a card list and resolves it to rectangles plus an interactive-control map; `controls.rs` draws (card rows, toggle switch, buttons, hotkey/combo fields, status pill, banner, focus rings); `state.rs` performs the actions (autosave + `WM_USER+100` reload post on every change, capture/restore, reset, autostart registry toggle).

- **Nav rail**: fixed 240dip left rail, Win11 pill indicator on the selected item.
- **Scrolling**: content column scrolls (wheel honors `SPI_GETWHEELSCROLLLINES`; PgUp/PgDn/Home/End; draggable overlay thumb).
- **Keyboard**: Tab/Shift+Tab cycle a flat focus list (nav items then content controls); Enter/Space activate; focus rings render only for keyboard-driven focus. UI Automation is deliberately deferred — keyboard completeness and high-contrast support are the compensating measures; if Narrator support is ever needed, the path is an `IRawElementProviderSimple` tree over the control map.
- **Combo dropdown** (the one extra HWND): a `WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW | WS_EX_TOPMOST` popup reusing the window recipe (dark attr, `DWMWA_WINDOW_CORNER_PREFERENCE` round). Because it never activates, the main window keeps focus and all keyboard routing; dismissal needs no hooks — outside presses in the main window, `WM_NCLBUTTONDOWN`, deactivation, Esc, or scrolling all close it.

## 4. Hotkey Recorder

The recorder must capture combos that are *already registered global hotkeys* (the running daemon owns them via `RegisterHotKey`) and bare `Win+X` combos the shell would eat. Focused `WM_KEYDOWN` sees neither, so while a field is recording, `recorder.rs` installs a **capture-scoped `WH_KEYBOARD_LL` hook**:

- The callback does the minimum allowed in an LL hook (same discipline as the daemon's Win+Tab hook): modifier keydowns pass through (keeping async key state truthful), any other keydown is re-posted to the settings window and swallowed, and a no-op `keybd_event(0xFF)` is injected when Win is held so Start doesn't open on release.
- The hook lives only during capture: commit, Esc-cancel, focus loss (`WM_ACTIVATE`/`WM_KILLFOCUS`), or window destruction uninstall it. If installation fails, recording falls back to plain `WM_KEYDOWN`.
- Translation (modifier snapshot + VK → `Hotkey`, bare-modifier rejection, Esc semantics) is a pure function with unit tests; committed hotkeys flow through `Config` normalization untouched.

## 5. Why Not a UI Framework

The settings window is ~2,000 lines of Rust and adds ~30 KB to the size-optimized binary. Any framework alternative would multiply the payload by orders of magnitude, add a second toolchain, and re-introduce a parallel config model that must mirror `winspaces_common::Config`. The hand-drawn approach keeps the entire product one dependency-free native exe — instant startup, a ~2 MB installer, and a single source of truth for the config schema.
