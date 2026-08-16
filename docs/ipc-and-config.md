# IPC Protocol & Configuration Schema

This document specifies the two cross-process contracts in WinSpaces: the Win32 message-based IPC between the daemon, the settings window, and CLI invocations; and the `settings.json` schema. Both the daemon and the settings process link `crates/winspaces-common`, so the constants and the config type have a single definition — there is no second implementation to keep in sync.

---

## 1. Daemon Discovery & Single Instance

The daemon owns a hidden message-only style window (a zero-sized `WS_POPUP` + `WS_EX_TOOLWINDOW` window) that doubles as the single-instance marker and the IPC endpoint:

- **Class**: `WinSpacesMessageClass` (`WINSPACES_MSG_WINDOW_CLASS`)
- **Title**: `WinSpacesMessageWindow` (`WINSPACES_MSG_WINDOW_TITLE`)

Any process locates the running daemon with `FindWindowW(class, title)`. Control commands (`--exit`, `--mission-control`) **never** boot a new daemon — if `FindWindowW` returns null they log a warning and return.

## 2. IPC Messages

All IPC is fire-and-forget `PostMessageW` to the message window. There are no replies; the settings window observes effects through the config file and the visible space state.

| Message | Value | Sender | Effect |
| :--- | :--- | :--- | :--- |
| `WM_WINSPACES_RELOAD_CONFIG` | `WM_USER + 100` | Settings window after saving `settings.json` | Re-reads config, re-registers hotkeys, applies taskbar mode |
| `WM_WINSPACES_CAPTURE_WORKSPACE` | `WM_USER + 101` | Settings "Capture" button | Snapshots current window layout into `workspace_rules`, saves config |
| `WM_WINSPACES_RESTORE_WORKSPACE` | `WM_USER + 102` | Settings "Restore" button | Applies `workspace_rules` to matching windows |
| `WM_WINSPACES_TOGGLE_MISSION_CONTROL` | `WM_USER + 103` | `winspaces.exe --mission-control`, LL keyboard hook, tray click | Toggles the Mission Control overlay |
| `WM_WINSPACES_RETILE` | `WM_USER + 104` | Internal scheduler, hook events | Arms debounced timer to retile visible dirty spaces |
| `WM_WINSPACES_TILING_TOGGLE` | `WM_USER + 105` | Settings window, CLI | Toggles dynamic tiling on or off and persists state |
| `WM_COMMAND` (`ID_TRAY_EXIT`) | — | `winspaces.exe --exit` | Graceful shutdown: restore all windows, remove tray icon, exit |

Capture is asynchronous from the settings window's perspective: after posting `CAPTURE_WORKSPACE` it waits ~300 ms (timer) before re-reading `settings.json` to pick up the new rules.

### UIPI (User Interface Privilege Isolation)

When the daemon runs elevated (the opt-in posture, §5) while the settings window and CLI invocations run at medium integrity, Windows silently drops messages sent from a lower to a higher integrity level. At startup the daemon therefore opts the message window in via `ChangeWindowMessageFilterEx(hwnd, msg, MSGFLT_ALLOW)` for the six `WM_WINSPACES_*` messages **and** `WM_COMMAND`. The filter is harmless when the daemon runs non-elevated, but removing it breaks config reload and `--exit` in the elevated-daemon case — with no error anywhere, because `PostMessageW` still reports success to the sender.

## 3. CLI Flags

`winspaces.exe` with no arguments starts the daemon (single instance enforced by the message window). Control flags:

| Flag | Behavior |
| :--- | :--- |
| `--exit` / `--kill` | Posts graceful shutdown to the running daemon; no-op if none |
| `--mission-control` / `-m` | Toggles Mission Control in the running daemon; no-op if none. Pinnable to the taskbar as a shortcut |
| `--tiling-toggle` / `-t` | Toggles dynamic window tiling on or off in the running daemon; no-op if none |
| `--settings` | Opens the native settings window ([`settings-ui.md`](settings-ui.md)) in this process — unlike the control flags above it does not message the daemon, it *is* the app. Single-instance: focuses an already-open settings window instead |
| `--dump [file]` | Diagnostic: writes all window metrics to `window_dump.txt` (or `file`) and exits |

Environment variables (read once at startup):

| Variable | Behavior |
| :--- | :--- |
| `WINSPACES_NO_SHELL_CLOAK=1` | Disables the ImmersiveShell cloak backend so taskbar mode uses the `SW_FORCEMINIMIZE` fallback ([`dwm.md`](dwm.md) §5.6). Testing escape hatch |

## 4. `settings.json` Schema

### Location & Portable Mode

`Config::get_config_path()` resolution order:
1. **Portable**: `settings.json` next to `winspaces.exe`, if it exists.
2. **Default**: `%LOCALAPPDATA%\WinSpaces\settings.json` (directory created on demand).

### Shape

```json
{
  "show_all_taskbar": false,
  "auto_restore_workspaces": false,
  "intercept_win_tab": true,
  "space_indicator": true,
  "mission_control": { "modifiers": 2, "vk": 38 },
  "switch_spaces":   [ { "modifiers": 1, "vk": 49 }, ... ],
  "move_spaces":     [ { "modifiers": 3, "vk": 49 }, ... ],
  "prev":      { "modifiers": 1, "vk": 37 },
  "next":      { "modifiers": 1, "vk": 39 },
  "move_prev": { "modifiers": 13, "vk": 37 },
  "move_next": { "modifiers": 13, "vk": 39 },
  "toggle_sticky": { "modifiers": 7, "vk": 80 },
  "tiling": {
    "enabled": false,
    "inner_gap": 8,
    "outer_gap": 12,
    "ratio_step_pct": 5,
    "toggle": { "modifiers": 7, "vk": 84 },
    "focus_left": { "modifiers": 7, "vk": 37 },
    "focus_right": { "modifiers": 7, "vk": 39 },
    "focus_up": { "modifiers": 7, "vk": 38 },
    "focus_down": { "modifiers": 7, "vk": 40 },
    "swap_left": { "modifiers": 14, "vk": 37 },
    "swap_right": { "modifiers": 14, "vk": 39 },
    "swap_up": { "modifiers": 14, "vk": 38 },
    "swap_down": { "modifiers": 14, "vk": 40 },
    "ratio_shrink": { "modifiers": 7, "vk": 189 },
    "ratio_grow": { "modifiers": 7, "vk": 187 },
    "toggle_float": { "modifiers": 7, "vk": 70 },
    "float_rules": [
      {
        "name": "Calculator",
        "aumid": "Microsoft.WindowsCalculator_8wekyb3d8bbwe!App",
        "exe_path": "",
        "class_name": "",
        "title_pattern": ""
      }
    ]
  },

  "workspace_rules": [
    {
      "name": "Brave (Work)",
      "aumid": "Brave.Profile1",
      "exe_path": "C:\\...\\brave.exe",
      "class_name": "Chrome_WidgetWin_1",
      "title_pattern": "",
      "display_index": 0,
      "space_index": 3,
      "show_cmd": 1,
      "rect": { "left": 0, "top": 0, "right": 1920, "bottom": 1040 },
      "is_snapped": false,
      "is_sticky": false
    }
  ]
}
```


### Hotkey Encoding

`Hotkey` fields map directly onto `RegisterHotKey` parameters:
- `modifiers`: bitwise OR of `MOD_ALT = 0x1`, `MOD_CONTROL = 0x2`, `MOD_SHIFT = 0x4`, `MOD_WIN = 0x8`. Unknown bits are masked off on load.
- `vk`: Win32 virtual-key code (`0x31` = `1`, `0x25` = Left, `0x26` = Up, ...). `vk: 0` means unassigned; the hotkey is not registered.

### Float Rule Fields

- `name`: friendly label for display in settings.
- `aumid` / `exe_path` / `class_name` / `title_pattern`: window matchers evaluated via the same `score_rule` specificity algorithm as workspace rules. Windows matching any rule remain floating across spaces and restarts.

### Workspace Rule Fields


- `aumid` / `exe_path` / `class_name` / `title_pattern`: window matchers scored per the fingerprint hierarchy in [`dwm.md`](dwm.md) §4 (AUMID is exact-match; title is one-directional contains).
- `display_index` / `space_index`: zero-based target monitor and space.
- `show_cmd`: `ShowWindow` command captured at snapshot time (`1` = normal, `3` = maximized).
- `rect`: target visible frame; for maximized rules it also seeds `rcNormalPosition` so un-maximizing lands on the right monitor.
- `is_snapped`: apply the DWM shadow-margin expansion + square-corner treatment from [`dwm.md`](dwm.md) §3.
- `is_sticky`: pin the window to every space of `display_index` (see [`mission-control.md`](mission-control.md)). Restored *after* `track_window`, never before — `SpaceManager::set_sticky` refuses an untracked window, because a pin held on a window that sits in no space list is invisible to every sweep that would act on it.

### Normalization Contract (crash-proofing)

The daemon **never trusts the file shape**. `Config::normalize()` runs on every load:
- `switch_spaces` / `move_spaces` are resized to exactly `MAX_SPACES` (9) entries — hotkey registration indexes these lists directly and must not panic on a short array. Missing tail entries are padded with the per-index *defaults* (`Alt+5..9` / `Ctrl+Alt+5..9`), so a settings.json written when there were only four spaces upgrades to working bindings; explicit `vk: 0` entries inside the stored length are the user's unbindings and survive. Only hotkeys up to the highest live space count across monitors are actually registered.
- Modifier bits outside the known mask are cleared.
- Unknown/missing optional fields fall back via serde defaults. Optional *hotkeys* added after release name a default function rather than taking `Hotkey::default()` — `toggle_sticky` is the live example. A bare `#[serde(default)]` there yields `{0, 0}`, which registers nothing, so every pre-existing settings.json would leave its owner as the only user without the binding a fresh install ships with.

An **unparseable** file is renamed to `settings.json.bak` (never silently overwritten — it may hold captured workspace rules) and defaults are written in its place.

### Single Source of Truth

`winspaces_common::Config` is the only definition of the schema, defaults, and normalization. The daemon and the settings window are the same binary, so every consumer gets identical behavior by construction — schema changes happen in exactly one place (`crates/winspaces-common/src/config.rs`). The IPC message constants used throughout this document live in the same crate's `ipc` module.

Both `settings.json` and `layouts.json` are written through `write_json_atomic` (temp file + rename), so a crash mid-write can never truncate either file.

## 5. `layouts.json` Schema

Written and read only by the daemon; not user-facing and not editable from the settings window. Lives beside `settings.json` (same portable/`%LOCALAPPDATA%` resolution, via `config_dir()`). Full rationale in [`display-topology.md`](display-topology.md).

One entry per **display topology signature** — the sorted, `|`-joined stable monitor device paths of an attached monitor set. Capped at 8 topologies, evicting the least recently captured.

```json
{
  "topologies": [
    {
      "signature": "\\\\?\\DISPLAY#BNQ805B#...|\\\\?\\DISPLAY#HWP2956#...",
      "captured_unix": 1786000000,
      "monitors": [
        {
          "stable_id": "\\\\?\\DISPLAY#BNQ805B#5&1f33c64f&0&UID4354#{...}",
          "device": "\\\\.\\DISPLAY2",
          "rect": { "left": 0, "top": 0, "right": 3840, "bottom": 2560 },
          "work": { "left": 0, "top": 0, "right": 3840, "bottom": 2508 },
          "dpi": 144,
          "current_space": 0,
          "space_count": 4
        }
      ],
      "windows": [
        {
          "name": "Fork.exe (Fork)",
          "aumid": "",
          "exe_path": "C:\\...\\Fork.exe",
          "class_name": "HwndWrapper[Fork.exe;;...]",
          "title_pattern": "",
          "stable_monitor_id": "\\\\?\\DISPLAY#BNQ805B#...",
          "space_index": 2,
          "show_cmd": 1,
          "is_snapped": false,
          "is_sticky": false,
          "rect": { "left": 366, "top": 537, "right": 2882, "bottom": 1950 },
          "rel": { "x": 0.095, "y": 0.214, "w": 0.655, "h": 0.563 },
          "dpi": 144
        }
      ]
    }
  ]
}
```

### Field Notes

- `stable_id` / `stable_monitor_id`: monitor device path from `QueryDisplayConfig`. Unlike `WorkspaceRule.display_index` (an enumeration ordinal) this survives RDP, docking and re-plugging.
- `space_count`: how many spaces the monitor had under this topology (serde default `4` for files written before counts were dynamic). Applied at startup and reconcile *regardless* of the auto-restore setting — counts are structural, not layout — and written directly (bypassing the shadow debounce) whenever the user adds or removes a space, so a count change with zero windows open still persists.
- `rect` **and** `rel`: absolute physical pixels for a pixel-exact replay onto an unchanged monitor; work-area fractions for a monitor that returned at a different resolution or scale. `dpi` decides which is used.
- The first four fields mirror `WorkspaceRule`'s matchers so `score_rule` matches snapshots without a second implementation.
- `is_sticky`: the pinned-to-every-space flag, and the *only* thing that carries a pin across a daemon restart. There is deliberately no window state-prop bit for it — `SpaceManager::new` runs `reclaim_orphaned_windows`, which zeroes every prop it finds, before the first scan, so a prop could never be read back anyway.

### Failure Contract

An unparseable `layouts.json` deserializes to an empty store — "no known topologies" until the next capture. It is never backed up or repaired: unlike `settings.json` it holds no user intent, and the next 5-second shadow tick regenerates it.

## 5. Elevation Posture

**WinSpaces runs non-elevated by default.** This is the shipping posture and the one the standard autostart uses (the settings window's autostart toggle writes an HKCU `Run` entry, which always launches at medium integrity). All core features — DWM cloaking, Mission Control, space switching, hotkeys, IPC — work at medium integrity; verified in day-to-day use.

Accepted, documented limitations of the non-elevated daemon:

- **Windows of elevated applications are unmanaged**: `DwmSetWindowAttribute(DWMWA_CLOAK)` and the `SW_HIDE` fallback both fail across integrity levels ([`dwm.md`](dwm.md) §5.5). Such windows simply stay visible on every space.
- **`Win+Tab` interception pauses while an elevated window has focus**: UIPI withholds low-level keyboard hook events from a lower-integrity process while a higher-integrity window is in the foreground. Interception resumes when focus returns to a normal window.

**Opt-in elevated mode** for users who need elevated apps managed: `scripts/install-elevated-autostart.ps1` (run once from an elevated shell) registers a logon scheduled task with `RunLevel Highest`, which starts the daemon elevated at login **without a UAC prompt**. The script removes the HKCU `Run` entry to avoid a double start, and the daemon itself carries a single-instance guard (§1) as a backstop. `-Remove` uninstalls the task. The installer (distribution work) must expose this as an optional feature, defaulting to off.

`dev run` performs no elevation of its own by default — the daemon inherits the integrity level of the terminal that launches it (use `dev run --admin` to launch elevated via UAC prompt from a non-elevated terminal).
