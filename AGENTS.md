# AGENTS.md

Guidance for AI agents working in this repository: what the product is, how
the crates fit, how to build and verify, where the documentation lives, and
the rules every commit follows. Read [`docs/README.md`](docs/README.md) next
for the page index and reading order.

## Project

WinSpaces is a per-monitor independent spaces manager for Windows. Windows'
own virtual desktops move every monitor together; WinSpaces gives each
display its own set of spaces (1-9, dynamic), like macOS "Displays have
separate Spaces", plus a native Mission Control overlay, an optional dwindle
tiling engine, workspace layout rules that survive monitor and RDP churn, and
a hand-drawn Windows 11-style settings window. **Windows-only.** One native
binary, `winspaces.exe`, built on raw Win32 FFI (`windows-sys`); no runtimes.
The feature list a user sees is in [`README.md`](README.md); the user-facing
manual is [`docs/user-guide.md`](docs/user-guide.md).

## Architecture

Five crates in strict one-way dependency order; nothing depends upward:

```
winspaces (bin) -> winspaces-ui -> winspaces-core -> winspaces-win32 -> winspaces-common
```

- **`winspaces-common`**: the config and layout schema (`Config`,
  `WorkspaceRule`, `LayoutStore`), the IPC message constants, the i18n tables
  and the logger. Single source of truth for both processes.
- **`winspaces-win32`**: an FFI kit with no product knowledge: GDI/DWM/DPI
  drawing primitives, window-class registration, the low-level hooks, the
  registry helpers, and the ImmersiveShell shell-cloak COM surface.
- **`winspaces-core`**: the daemon's non-UI logic: `SpaceManager` (space
  tracking, the show/hide state machine, rule placement, re-homing, the
  activation decision), the tiling engine, workspace capture and matching,
  display topology identity, layout persistence, and hotkeys. No rendering,
  no `AppState`.
- **`winspaces-ui`**: the owner-drawn surfaces as peers over one crate-level
  theme: tray icon and acrylic context menu, Mission Control, the settings
  window, the transient space indicator, and the tiling split preview.
- **`winspaces`** (bin): CLI dispatch, the message loop, `AppState`, and the
  `McHost` vtable of `fn` pointers that lets Mission Control act on daemon
  state it cannot name.

[`docs/crate-layout.md`](docs/crate-layout.md) is the per-crate map and
holds the `windows-sys` feature rule; the domain pages explain each
mechanism.

A sixth crate, `winspaces-bench`, sits outside this chain rather than atop
it: a console tool, never shipped, that depends on the four library crates
to drive and measure the daemon binary ([`docs/crate-layout.md`](docs/crate-layout.md)
§1-§2, [`docs/benchmarks.md`](docs/benchmarks.md)).

One binary, two process roles. The **daemon** (default invocation) owns the
tray icon, the global hotkeys, the WinEvent and keyboard hooks, the DWM and
shell cloaking that hides a space's windows, Mission Control, and a hidden
message window that doubles as single-instance marker and IPC endpoint. The
**settings window** (`winspaces.exe --settings`) is a separate process of the
same exe, so a settings crash never takes the daemon down and the daemon pays
nothing for the settings code while it is closed. Control flags (`--exit`,
`--mission-control`, `--tiling-toggle`, `--restart`, the elevation flags,
`--dump`) are short-lived invocations that message the running daemon; the
complete table is [`docs/ipc-and-config.md`](docs/ipc-and-config.md) §3, the
IPC messages are in §2.

Invariants an agent must not break; each is explained where it lives:

- `track_window` is the only mutation of space membership, and a window's
  hidden-state prop is never cleared while a cloak is physically applied
  ([`docs/dwm.md`](docs/dwm.md) §5.3).
- `switch_space` is the single choke point that notifies the space indicator
  ([`docs/space-indicator.md`](docs/space-indicator.md)).
- `with_app_state` drops re-entrant events instead of queueing them, and the
  Mission Control drag state machine depends on which events get dropped
  (`crates/winspaces/src/hostfns.rs`).
- Mission Control calls its host only after the `MC_STATE` borrow is
  released; `SetCapture` happens inside it
  (`crates/winspaces-ui/src/mission_control/input.rs`).
- A drag gesture commits on its meaningful axis and is never cancelled
  because the cursor strayed.
- The tray icon toggles Mission Control on a single click; no double-click,
  no delay.

## Build & Run

A `dev` task runner (`dev.ps1` + `dev.cmd` shim) wraps every build and
execution task. `dev.ps1` sets `Set-StrictMode -Version Latest`, which is
*dynamically* scoped: every script it invokes under `scripts/` inherits it.
PowerShell unrolls a single-element array to a scalar on return, so
`(Get-Thing).Count` throws there when exactly one item comes back; write
`@(Get-Thing).Count`. It fails only in the one-item case, so it survives
casual testing.

```powershell
.\dev build             # cargo build --workspace (winspaces.exe)
.\dev run               # daemon, non-elevated (inherits the terminal's integrity)
.\dev run --admin       # daemon elevated (UAC prompt)
.\dev run settings      # the native settings window
.\dev check             # fmt, clippy --all-targets, tests, cargo check -p per crate, features
.\dev features          # the windows-sys feature audit on its own
.\dev dist              # the installer into dist\ (signed if WINSPACES_SIGN_THUMBPRINT is set)
.\dev release <x.y.z>   # bump the version, roll CHANGELOG.md, commit and tag
.\dev recover           # stop the daemon, then restore hidden/cloaked windows
.\dev bench <group>     # micro/primitives/static/live/all/smoke/compare/ab
.\dev site              # serve site\ (winspaces.app) on http://127.0.0.1:8338
```

`dev build` and `dev run` stop a daemon that runs from this repo's target
exe with a graceful `--exit` first and never force-kill it; a daemon running
elevated must be stopped from an elevated terminal.

## Contributor contract

- Run `.\dev check` before every commit. It is exactly what
  `.github/workflows/ci.yml` runs on every push and pull request, and what
  `release.yml` runs before packaging a tag.
- `dev check` runs the benchmark smoke; a benchmark that stops compiling
  fails CI.
- Every crate declares every `windows-sys` feature its own source uses
  (`.\dev features`). `cargo check -p <crate>` cannot prove this on its own
  because features unify upward from the crate's dependencies
  ([`docs/crate-layout.md`](docs/crate-layout.md) §3).
- Commits are one line, conventional (`type(scope): subject`), imperative,
  no body. No reference to AI assistance anywhere in the repository.
- A behaviour-preserving refactor and a behaviour change never share a
  commit. A change that no unit test can prove names its manual check.
- Update the page that owns the mechanism in the same commit and bump its
  `Last verified` line. Performance numbers go only in
  [`docs/benchmarks.md`](docs/benchmarks.md).
- User-visible text is a key in `crates/winspaces-common/locales/*.json`,
  never a literal ([`docs/i18n.md`](docs/i18n.md)).
- Never drive the live daemon by injecting input or stealing focus; take
  screenshots with `PrintWindow` and ask for interactive checks.

## Documentation

- [`docs/README.md`](docs/README.md): index, reading order, the list of
  "rejected alternatives" sections, and the page conventions.
- [`docs/user-guide.md`](docs/user-guide.md): the manual for the person
  running the app, including troubleshooting.
- [`reports/`](reports): point-in-time records (reviews, investigations,
  research, checklists). Each starts with a `Status:` line; they are never
  authoritative over `docs/`.
- [`CHANGELOG.md`](CHANGELOG.md): keep-a-changelog; `dev release` rolls the
  *Unreleased* section.
- [`site/`](site): the winspaces.app landing page, plain static files
  deployed by Cloudflare Pages from `main` (output directory `site`);
  `dev site` previews it. [`assets/`](assets): the logo and icon, generated
  by `scripts/gen-icon.py` — edit the spec there, never the outputs.

## Runtime artifacts

Portable mode wins when `settings.json` sits next to the exe; otherwise
everything lives in `%LOCALAPPDATA%\WinSpaces\`: `settings.json`,
`layouts.json` (one layout per monitor topology), and `winspaces.log`
(rotated at 5 MiB to `winspaces.log.old`; `WINSPACES_LOG=debug` raises the
level). A file that does not parse is moved to `.bak` and replaced by
defaults, never silently overwritten. Schemas and normalisation:
[`docs/ipc-and-config.md`](docs/ipc-and-config.md) §4 and §5.

If a build leaves windows cloaked after exit, `scripts/recover-windows.ps1`
(`dev recover`) uncloaks every top-level window and re-shows the ones
WinSpaces was tracking. It stops the daemon first, and that order is
load-bearing: the sweep clears the per-window state prop, and a daemon
running underneath would keep tracking windows it can no longer hide or
show. Why the prop is the recovery contract: [`docs/dwm.md`](docs/dwm.md)
§5.3. An elevated daemon needs the script run elevated too.
