# Crate Layout

WinSpaces builds from five crates. This page is the map: the dependency
graph, what belongs where, and the one rule that keeps a per-crate build
honest. For the *why* behind specific architectural choices — the Overview
host indirection, the tray/settings theme sharing, the DWM COM
thread-affinity invariant — see the domain docs linked from
[`AGENTS.md`](../AGENTS.md); this page only covers structure.

*Last verified: 2026-09-12, against 9f2a056.*

---

## 1. Dependency Graph

```
winspaces        (bin)   -> winspaces-ui, winspaces-core, winspaces-common
winspaces-ui             -> winspaces-win32, winspaces-core, winspaces-common
winspaces-core           -> winspaces-win32, winspaces-common
winspaces-win32          -> winspaces-common
winspaces-common

winspaces-bench          -> winspaces-ui, winspaces-core, winspaces-win32, winspaces-common
```

`winspaces-bench` is a sixth crate, a leaf consumer sitting beside the bin
rather than in its chain: it depends on all four library crates to drive and
measure the daemon, nothing depends on it, and it is never shipped (§2 below).

**Nothing depends upward.** `winspaces-common` knows nothing about Win32 UI;
`winspaces-win32` knows nothing about spaces; `winspaces-core`
knows nothing about rendering; `winspaces-ui` knows nothing about `AppState`.
Verify with `cargo tree -p <crate>` — a lower crate must never list a higher
one, and `cargo tree -e no-dev` across the workspace must show no cycles.

The bin, `winspaces` (package and binary both named `winspaces`, producing
`winspaces.exe`), sits alone at the top. It is intentionally kept a thin
binary rather than split into a library-plus-bin: the four crates below it
already deliver the reusable, independently-testable surface a lib split
would buy, and what remains in the bin — `AppState`, the message loop,
startup sequencing — is HWND/message-loop glue that can't be tested headless
regardless of how it's packaged.

## 2. What Belongs in Each Crate

### `winspaces-common`

The schema and constants every other crate and process needs, with zero
Win32 UI surface:

- `config` — `Config`, `Hotkey`, `WindowRect`, `WorkspaceRule`, load/save,
  normalization. The single source of truth for `settings.json`'s shape. The
  serde defaults of every field live in `config/defaults`, one named
  function each so a config written before a field existed upgrades to the
  working value.
- `hotkey_label` — `hotkey_to_string` / `hotkey_to_string_in`, the display
  form of a binding in the UI language. Presentation, kept out of the schema
  module.
- `i18n` — every user-visible string, generated from `locales/*.json` by the
  crate's `build.rs` (the only build script in the workspace) into static
  per-language tables; `t`/`tr!`/`tn`, `Lang`, the current-language atomic.
  Lives here because both processes and every UI crate read it and nothing
  may depend upward. See [`i18n.md`](i18n.md).
- `ipc` — the `WM_WINSPACES_*` message IDs and the message-window class/title
  constants shared between the daemon, the settings process, and CLI
  invocations.
- `spaces` — `MAX_SPACES` / `DEFAULT_SPACES`.
- `paths` — config directory resolution, atomic JSON writes.
- `layout` — `LayoutStore`, `TopologySnapshot`, `MonitorSnapshot`,
  `WindowSnapshot`, `RelRect` — the `layouts.json` shape.
- `logger` — the file logger every other crate's `log_info!`/`log_warn!`/
  `log_error!` macros expand into. Declared `pub mod` on purpose: the macros
  expand to `$crate::logger::Logger::log`, which resolves in the *defining*
  crate, so a private module breaks every expansion in every consumer.

### `winspaces-win32`

Safe-ish FFI wrappers with no product knowledge — a thin kit, not a UI
toolkit. Roughly a third of it is genuinely shared drawing code; the rest is
`hooks` and `shell_cloak` moved verbatim because they're pure FFI with no
domain logic to extract.

- `text` — wide-string conversion (`encode_wide`, `wide_to_string`).
- `module` — `app_instance()`, `win_build()` (the cached `RtlGetVersion`
  build-number check every surface uses to gate acrylic/Mica availability).
- `window_class` — `register_class`, once-guarded per class name; unifies
  what used to be three separate `RegisterClassW` call sites.
- `dwm` — `set_dark_mode`, `set_round_corners`, `extend_frame_full`,
  `set_backdrop`. Owns the `DWMWA_WINDOW_CORNER_PREFERENCE` literal that
  `windows-sys` 0.59 doesn't export.
- `dpi` — per-monitor scale helpers.
- `display` — frame-interval timing, monitor-rect lookup.
- `gdi` — a table-of-contents module over:
  - `color` — `rgb`, `premultiply`, `Tint`.
  - `font` — `create_font` and the three shared font-face constants
    (`FACE_DISPLAY`, `FACE_TEXT`, `FACE_ICONS`).
  - `surface` — the two painting techniques, deliberately kept apart:
    `paint_surface` (premultiplied-alpha DIB, for the menu and the settings
    window) and `double_buffer` (opaque compatible bitmap, for
    Overview). See [`tray-and-menu.md`](tray-and-menu.md) §2 and
    [`overview.md`](overview.md) §1.
  - `draw` — rounded fills, text/glyph drawing, measurement.
  - `guard` — RAII wrappers (`MemDc`, `ScreenDc`, `GdiObject<T>`,
    `SelectGuard`, `DibSection`) for GDI resource lifetimes.
- `glyphs` — the Segoe Fluent Icons codepoint table, one definition shared by
  every surface that draws an icon glyph.
- `dialogs` — `open_file_dialog` / `save_file_dialog` over `comdlg32`, owning
  the `OPENFILENAMEW` buffer and double-null filter contract so callers pass
  plain `&str` and get a `PathBuf` back. Both spin a nested modal loop, so a
  caller must not hold a `RefCell` borrow across them (see
  [`settings-ui.md`](settings-ui.md) §3).
- `hooks` — `WinEventHook`, `KeyboardHook` (verbatim move).
- `registry` — the HKCU read/write/delete helpers behind the theme
  preference, the autostart `Run` value and the OS apps-theme flag.
- `security` — `is_current_process_elevated`.
- `shell_cloak` — the ImmersiveShell `IApplicationView::SetCloak` COM surface
  (verbatim move; see [`dwm.md`](dwm.md) §5.6 for its thread-affinity
  invariant).

`winspaces-win32` deliberately ships **no shared `pt_in_rect`**: Overview's
hit test is inclusive of the right/bottom edge, the menu's and the
settings window's are exclusive, and unifying them would silently shift real
hit targets by a pixel. Three call sites, not one shared helper with a
footgun. Similarly there is no shared "surface" abstraction over
`paint_surface`/`double_buffer` — they're different techniques for different
reasons, documented as siblings, not variants of one thing.

### `winspaces-core`

The daemon's non-UI logic: space tracking, workspace placement,
display topology, hotkeys. No rendering, no `AppState`, no knowledge that a
UI even exists.

- `spaces` — `SpaceManager` and the per-monitor space state machine. One
  struct, many `impl` blocks, one file per concern: `manager` (the struct,
  tracking, scanning, switching), `state` (the `SetProp` window-property
  contract), `eligibility` (`is_valid_window`/`is_eligible`, pure),
  `index_math` (pure index-remapping math for reorder/removal), `monitor`
  (`MonitorState`, monitor enumeration), `visibility` (the DWM-cloak /
  shell-cloak / forced-minimize state machine, [`dwm.md`](dwm.md) §5),
  `count_ops` (add/remove/reorder/set space counts), `sticky` (pinned
  windows), `rules` (workspace-rule placement for the startup and manual
  restores only), `rehome` (cross-monitor re-homing and adoption of untracked windows,
  with the guards every window source shares), `enforce` (the settle window
  and post-restore placement enforcement), `activation` (the decision behind
  "follow the user to the activated window's space"), and `notify` (the
  switch observer the space indicator hooks).
- `tiling` — the dynamic tiling engine ([`tiling.md`](tiling.md)): `types`
  (`TileSpace`, `Gaps`, `Direction`), `algorithms` (the pure dwindle layout,
  exactness-tested), `membership` (slot-order reconciliation, pure),
  `neighbors` (directional focus and swap, pure), `resize` (drag
  classification, pure), `apply` (the `DeferWindowPos` batch), `engine`
  (`impl SpaceManager`: retile stages, the verify sweep and its pure frame
  verdict, keyboard and drag operations), and `notify` (the retile scheduler
  the bin installs).
- `workspaces` — window fingerprinting and rule-based placement, split into
  `query` (process/class/AUMID/title lookups), `identity` (the cached window
  fingerprint), `placement` (`apply_rule_to_window`, snap detection),
  `matching` (`score_rule`, pure), `capture` (`capture_active_workspace`),
  and `dump` (the `--dump` diagnostic).
- `daemon` — `find_daemon_window` / `is_daemon_running`: how any process
  (settings window, a control flag, a second daemon) finds the running one.
- `topology` — stable monitor identity and topology signatures (see
  [`display-topology.md`](display-topology.md) §2).
- `layout_store` — shadow/persist/restore of per-topology layouts (see
  [`display-topology.md`](display-topology.md) §4).
- `hotkeys` — `RegisterHotKey` management over `Config`, the hotkey id
  layout, and `decode_hotkey` → `HotkeyAction`, so the bin only dispatches.

### `winspaces-ui`

The owner-drawn surfaces, as peers sharing one theme and one drawing kit
rather than independent implementations:

- `theme` — the crate-level palette resolution (light/dark/system, DWM
  accent, high contrast) both the tray menu and the settings window read
  from. Promoting this out of the settings module and up to crate level is
  what let the menu stop reaching into settings-owned code — see
  [`tray-and-menu.md`](tray-and-menu.md) §2.
- `tray` — `TrayIcon` lifecycle (`Shell_NotifyIconW`), plus `tray/badge` for
  the runtime-generated Fluent badge icon.
- `menu` — the acrylic context-menu flyout, split into `layout` (metrics,
  hit-testing), `render` (painting, built on `winspaces_win32::gdi::surface`),
  `input` (interaction, submenus, timers), and `legacy` (the pre-Win11
  `HMENU` fallback frontend).
- `overview` — the live window thumbnails overlay: state and the `OverviewHost` vtable at
  the crate root, `geometry` (pure hit-testing/layout math, unit-tested),
  `cards` (DWM thumbnail registration and font/icon setup), `render`,
  `input`. See [`overview.md`](overview.md) §1.1 for why it
  takes `&mut SpaceManager` plus a host vtable instead of `AppState`.
- `space_indicator` — the transient "Space N" panel shown on a switch, with
  its pure `geometry` submodule (placement plus the anti-aliased rounded-rect
  coverage that shapes it). The one layered, backdrop-free surface in the
  crate; see [`space-indicator.md`](space-indicator.md) §2 for why.
- `settings` — the native settings window, split into `layout`, `render`,
  `actions`, `combo` (the one child-HWND popup), `wndproc`, `controls`
  (Fluent chrome built on the `winspaces-win32` primitives), `pages` (the
  shared item types; `pages/spec` describes each page's cards, pure over
  `Config` and the language; `pages/resolve` turns them into rectangles),
  `state`, `recorder`, `autostart`.
- `tiling_preview` — the translucent ghost overlay that previews a flipped
  split during a Shift-drag ([`tiling.md`](tiling.md) §4).

### `winspaces` (the bin)

CLI dispatch, the message loop, `AppState`, and the glue that lets the crates
below act on state they cannot otherwise reach:

- `app` — `AppState`, `with_app_state`, tray-icon/menu-theming helpers. Not
  `pub`: nothing outside the bin can name `AppState`, which is the whole
  point of the `OverviewHost` indirection.
- `hostfns` — the `OverviewHost` implementation: nine `fn` items, each a verbatim
  wrapper around a `with_app_state` block, installed once at startup.
- `wndproc` / `handlers` — the window-procedure dispatch and its per-message
  handlers: `commands` (tray menu), `ipc` (the `WM_WINSPACES_*` messages),
  `session` (timers, display, session and power events), `shell` (the
  ShellHook messages and the activation path), `winevents` (the WinEvent
  hook procedures), `keyboard` (the low-level keyboard hook), and
  `drag_preview` (the Shift poll during a tiled drag).
- `elevation` — the `--enable-elevation` / `--disable-elevation` /
  `--restart` handlers and the scheduled-task definition.
- `spaces` — the `add_space_on`/`remove_space_on`/`reorder_space_on`/
  `after_space_count_change` choke points `hostfns` wraps.
- `shadow` / `restore` — the tick-driven topology shadow/reconcile/restore
  orchestration. Stays in the bin because it mutates `AppState` fields
  together; see [`display-topology.md`](display-topology.md) §4.
- `tray_menu` — builds the one `Vec<MenuEntry>` both the custom flyout and
  the legacy `HMENU` frontend render.

### `winspaces-bench` (the benchmark tool)

A console bin, never shipped and never depended on, that drives and measures
the daemon binary ([`benchmarks.md`](benchmarks.md) is what it measures and
`dev bench` is how it's run). It sits beside `winspaces`, not below it: both
depend on the same four library crates, but `winspaces-bench` has no path
from or to the bin.

What belongs here:

- `stats`, `timing`, `report`, `stamp` — the schema, the in-process sample
  runner, and the machine/build stamps every result carries.
- `micro`, `primitives`, `live`, `static_info` — the four measurement
  groups, one module each.
- `compare` — diffs two results and flags regressions.

What does not:

- **No product logic.** A benchmark exercises what the four library crates
  already expose; it never grows a parallel implementation of something
  `winspaces-core` or `winspaces-ui` already does.
- **Public API only**: no `cfg(test)` builder, no crate-internal hook
  added just to make a benchmark easier to write.
- **No `test-support` feature.** A workspace build unifies features across
  every crate being compiled (§3 below is the general form of this), so a
  feature flag that exists only to help `winspaces-bench` poke at internals
  would unify into `cargo build --workspace` and ship inside `winspaces.exe`
  itself — the exact thing a dev-only tool must never cause. If a benchmark
  needs something the public API doesn't expose, the answer is to expose it
  properly, not to grow a feature-gated backdoor.

## 3. The Per-Crate `windows-sys` Feature Rule

**Every crate declares every `windows-sys` feature it uses, in its own
`Cargo.toml`.** Never rely on a sibling crate having already enabled a
feature you need.

This matters because a workspace-wide build *unifies* features across every
crate being compiled — so a crate missing a feature declaration compiles
fine as part of the workspace and only fails when that crate is built or
depended on in isolation ([cargo#12562](https://github.com/rust-lang/cargo/issues/12562)).
That failure mode is silent until it isn't: it surfaces only when someone
runs `cargo check -p <crate>` alone, publishes the crate separately, or a
future crate depends on it without also happening to need the same features.

Two checks enforce it, both part of `dev check` and CI:

- `cargo check -p <crate>` for each of the five crates — each must exit 0 on
  its own. This is only conclusive for the crate at the bottom of the graph:
  `-p` still pulls the crate's *path dependencies* into the build, and their
  features unify upward, so `cargo check -p winspaces-ui` compiles even when
  `winspaces-ui` forgot `Win32_Graphics_Gdi` as long as `winspaces-win32`
  declares it.
- `dev features` (`scripts/check-features.ps1`) — reads every
  `windows_sys::Win32::…` path named in a crate's own source, maps it to the
  deepest module feature, and fails if the crate's `Cargo.toml` (plus what its
  declared features imply) does not cover it. That is the check that actually
  catches the omission above. It proves "declared ⊇ used", not minimality: a
  feature can be needed for a type that only appears in a signature
  (`RegCreateKeyExW` needs `Win32_Security` although no path names it), so the
  script lists "declared but not named" features as information, never as a
  failure.

Check each crate's `Cargo.toml` for its current feature list; this page
intentionally doesn't duplicate it; a duplicated list is exactly the kind of
detail that drifts unnoticed.

The workspace root's `Cargo.toml` centralizes the `windows-sys` *version*
(`[workspace.dependencies]`) so every crate stays on the same release; only
the feature set is per-crate.

## See also

- [`AGENTS.md`](../AGENTS.md) for the invariants and the contributor contract.
- [`ipc-and-config.md`](ipc-and-config.md) for the contracts the two processes share.
- [`overview.md`](overview.md) §1.1 for the `OverviewHost` indirection the bin installs.
