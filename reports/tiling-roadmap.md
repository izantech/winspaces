# Tiling Roadmap

Feature-gap research comparing the WinSpaces tiling engine (see [tiling.md](../docs/tiling.md))
against Hyprland, komorebi, GlazeWM, and i3/sway, with a ranked adoption shortlist.
Researched 2026-08-16 against hyprwm/Hyprland, the Hyprland wiki, LGUG2Z/komorebi
(docs and release notes through v0.1.41), glzr-io/glazewm, and sway(5).

## Where WinSpaces stands today

Everything below ships on `feature/window-tile-manager` and is live-tested:

- Dwindle auto-tiling (spiral BSP, exactness-tested pure layout function)
- Global toggle (hotkey + CLI + settings)
- Inner/outer gaps, DPI-scaled per monitor
- Directional focus and swap (4-way, keyboard)
- Ratio adjust (keyboard and border drag)
- Float toggle per window + float rules per app
- Mouse drag-swap with forgiving drop targets
- Resistance auto-float (strike system + flatten-attempt cap)
- Maximize flattening (the tiler owns geometry; float first to keep a window maximized)
- Topology resilience (RDP and monitor power cycles)
- Plus the pre-existing core: per-monitor spaces 1-9, Mission Control (covers what
  Hyprland needs a plugin for), sticky windows (Hyprland's `pin`), and workspace
  layout save/restore — which none of the four comparison managers have in
  comparable form.

## The landscape at a glance

Features the reference managers have that WinSpaces currently does not, deduplicated.

| Feature | Found in | What it does | Tier |
| --- | --- | --- | --- |
| Monocle mode | komorebi | Focused window temporarily fills the work area; others keep slots; toggle restores | **adopt** |
| Focused-tile border | komorebi, all Linux WMs | Colored border marks the focused window | **adopt** |
| Smart gaps | Hyprland, GlazeWM, sway | Suppress gaps when a space has a single window | **adopt** |
| Cycle focus / cycle swap | komorebi, sway | Next/previous window in slot order, wrapping | **adopt** |
| Space back-and-forth | i3/sway, GlazeWM, Hyprland | Re-pressing the current space hotkey returns to the previous space | **adopt** |
| Pseudotile | Hyprland | Window keeps preferred size, centered in its slot | plan |
| Master-stack layout | Hyprland, komorebi, dwm | Main window + stack column; promote hotkey | plan |
| Persisted split ratios | komorebi (quicksave) | Ratios survive daemon restarts | plan |
| Scratchpad / special space | Hyprland, i3/sway | Hotkey summons a designated window as an overlay from any space | plan |
| Open-on-space rules | all four | App identity → always open on space N / monitor M | plan |
| Mouse-follows-focus | komorebi | Cursor warps to the keyboard-focused window | plan |
| Swap spaces between monitors | Hyprland | Exchange the active spaces of two monitors | plan |
| Focus-follows-mouse | komorebi, all Linux WMs | Hovering a window focuses it | plan |
| Split-direction control | Hyprland (`togglesplit`) | Override the longer-side split heuristic | parked |
| Window groups / tabs | Hyprland groups, komorebi stacks, i3 tabbed | Multiple windows share one tile with a tab bar | parked |
| Per-space tiling toggle | komorebi, GlazeWM | Tiling on/off per workspace | parked |
| Binding modes / submaps | i3/sway, GlazeWM, Hyprland | A "resize mode" that temporarily rebinds arrows | parked |
| Animations | Hyprland, komorebi | Animated retile transitions | skip |
| Status bar | GlazeWM/Zebar, waybar | Workspace indicator bar | skip |
| Blur / opacity effects | Hyprland | Compositor eye candy | skip |
| Custom grid layouts from files | komorebi | User-authored JSON layouts | skip |
| Scrolling layout | niri, PaperWM | Infinite horizontal column strip | skip |

## Adopt — the tiling v1.1 shortlist

High value, low-to-moderate effort; each drops cleanly into the existing engine.

### 1. Monocle mode

One hotkey makes the focused tile fill the work area; toggling back restores the
layout. This completes the maximize-flattening decision: native maximize was taken
away from tiled windows, and monocle is the tiler-owned replacement that cannot
fight restore enforcement. komorebi treats it as core (dedicated border color,
focus-cycling behavior).

*Fit:* `TileSpace.monocle: Option<HWND>`; `compute()` returns a single full-area
rect for it while `order` is preserved; the verify pass already handles the rest.
One hotkey + a settings row. No new Win32 surface.

### 2. Focused-tile border highlight

With flush tiles and zero gaps, nothing shows which window has focus — every
tiling WM solves this with a colored border, and it is komorebi's most visible
feature.

*Fit:* Windows 11 exposes `DWMWA_BORDER_COLOR` (attribute 34) — set an accent on
the focused tile and `DWMWA_COLOR_DEFAULT` on blur, driven from the existing
`EVENT_SYSTEM_FOREGROUND` hook. One `DwmSetWindowAttribute` call per focus change,
no overlay windows (unlike komorebi's implementation), lives in
`winspaces-win32::dwm`. Config: enable + color.

### 3. Smart gaps

With gaps configured, a lone window on a space sits inside a pointless gutter.
Hyprland (`no_gaps_when_only`), GlazeWM (`single_window_outer_gap`), and sway
(`smart_gaps`) all special-case n=1.

*Fit:* a branch in `compute_dwindle` when `n == 1`, plus one config bool. Pure
function, unit-testable.

### 4. Cycle focus / cycle swap

Directional focus is precise but slow for "just give me the next window."
komorebi's `cycle-focus`/`cycle-move` wrap through slot order with one repeated
keystroke.

*Fit:* `order` is already an ordered `Vec` — next/prev with wraparound is index
arithmetic. Two hotkeys plus shifted swap variants. Smallest item on this list.

### 5. Space back-and-forth

Pressing the hotkey for the space you are already on bounces to the previously
focused space. i3 muscle memory; GlazeWM ships it as `toggle_workspace_on_refocus`.
Benefits WinSpaces overall, not just tiling.

*Fit:* `MonitorState.previous: usize` updated in `switch_space`; the
switch-to-current-space hotkey path redirects to it. Config bool, default on.

## Plan — worth designing first

- **Pseudotile as the resistance response.** Hyprland centers a window at its
  preferred size inside its slot. The strike system currently *floats* resisters
  (min-size Chromium, elevated windows); centering them in-slot instead keeps the
  layout honest and reuses the same detection machinery. Needs design for the
  interaction with `expected`/verify. No Windows tiler does this.
- **Master-stack layout + promote.** Already scoped as the second layout — the
  `LayoutKind` enum and `compute()` dispatch were built for it. Pair with a
  promote-to-main hotkey (komorebi's `promote`, dwm's zoom).
- **Persist split ratios** in `TopologySnapshot` keyed by stable_id (the optional
  M4 item, currently documented as not persisted). Cheap, but touches snapshot
  serde and the `same_placement` exclusions.
- **Scratchpad space.** A designated app summoned/hidden as an overlay from any
  space. The cloak infrastructure makes this feasible; needs semantics design
  (space 0? per-monitor or global?).
- **Open-on-space rules.** Extend the rule vocabulary (float rules + workspace
  layouts exist) with "always open on space N." GlazeWM's command-style rules are
  the reference.
- **Mouse-follows-focus / focus-follows-mouse.** Both opt-in, default off. Cursor
  warping (`SetCursorPos`) is acceptable only as an explicit user preference. FFM
  needs a low-level mouse hook or polling; komorebi measured roughly 4% CPU for
  its implementation — treat as expensive.
- **Swap active spaces between monitors** (Hyprland `swapactiveworkspaces`).
  Natural fit for a dual-monitor desk; moderate churn in switch/show logic.

## Parked and skipped

**Parked by earlier scoping decisions** (revisit only deliberately): per-space
toggle (global-only was a deliberate choice), window groups/tabs (komorebi's
stacks prove it works on Windows, but the stackbar UI is a project of its own),
split-direction control, binding modes (the all-or-nothing hotkey registration
would need a mode-stack redesign), deep-split drag-resize, animations.

**Skipped as wrong for WinSpaces:** status bars (tray + Mission Control cover
it), blur/opacity compositing (not available to an out-of-process manager),
custom JSON layouts (dwindle + master cover the real use), scrolling layouts
(a different product), workspace overview plugins (Mission Control already
exists and is better integrated than anything in the comparison set).

## Recommendation

Merge `feature/window-tile-manager` as-is: the feature is coherent, and none of
the gaps are regressions — komorebi aside, WinSpaces already matches or beats
GlazeWM on tiling fundamentals and beats everything on session restore.

Then run a small **tiling v1.1** pass with the five adopt-tier items — roughly
one phase of work combined. Monocle and the focused border change daily feel the
most; smart gaps, cycle focus, and back-and-forth are near-free. Pseudotile-for-
resisters is the sleeper: it upgrades an existing mechanism rather than adding a
new one.
