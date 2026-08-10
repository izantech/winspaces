# IPC Protocol & Configuration Schema

This document specifies the two cross-process contracts in WinSpaces: the Win32 message-based IPC between the daemon, the GUI, and CLI invocations; and the `settings.json` schema shared by the Rust daemon and the C# configurator. Constants live in `crates/winspaces-common/src/lib.rs` and must stay in sync with `gui/WinSpaces.Gui/Services/IpcService.cs`.

---

## 1. Daemon Discovery & Single Instance

The daemon owns a hidden message-only style window (a zero-sized `WS_POPUP` + `WS_EX_TOOLWINDOW` window) that doubles as the single-instance marker and the IPC endpoint:

- **Class**: `WinSpacesMessageClass` (`WINSPACES_MSG_WINDOW_CLASS`)
- **Title**: `WinSpacesMessageWindow` (`WINSPACES_MSG_WINDOW_TITLE`)

Any process locates the running daemon with `FindWindowW(class, title)`. Control commands (`--exit`, `--mission-control`) **never** boot a new daemon — if `FindWindowW` returns null they log a warning and return.

## 2. IPC Messages

All IPC is fire-and-forget `PostMessageW` to the message window. There are no replies; the GUI observes effects through the config file and the visible desktop state.

| Message | Value | Sender | Effect |
| :--- | :--- | :--- | :--- |
| `WM_WINSPACES_RELOAD_CONFIG` | `WM_USER + 100` | GUI after saving `settings.json` | Re-reads config, re-registers hotkeys, applies taskbar mode |
| `WM_WINSPACES_CAPTURE_WORKSPACE` | `WM_USER + 101` | GUI "Capture" button | Snapshots current window layout into `workspace_rules`, saves config |
| `WM_WINSPACES_RESTORE_WORKSPACE` | `WM_USER + 102` | GUI "Restore" button | Applies `workspace_rules` to matching windows |
| `WM_WINSPACES_TOGGLE_MISSION_CONTROL` | `WM_USER + 103` | `winspaces.exe --mission-control`, LL keyboard hook, tray click | Toggles the Mission Control overlay |
| `WM_COMMAND` (`ID_TRAY_EXIT`) | — | `winspaces.exe --exit` | Graceful shutdown: restore all windows, remove tray icon, exit |

Capture is asynchronous from the GUI's perspective: after posting `CAPTURE_WORKSPACE` the GUI waits briefly (`Task.Delay`) before re-reading `settings.json` to pick up the new rules.

### UIPI (User Interface Privilege Isolation)

The daemon commonly runs elevated (`dev run` launches it as Admin) while the GUI and CLI invocations run at medium integrity. Windows silently drops messages sent from a lower to a higher integrity level, so at startup the daemon opts the message window in via `ChangeWindowMessageFilterEx(hwnd, msg, MSGFLT_ALLOW)` for the four `WM_WINSPACES_*` messages **and** `WM_COMMAND`. Removing this breaks config reload and `--exit` in the elevated-daemon case — with no error anywhere, because `PostMessageW` still reports success to the sender.

## 3. CLI Flags

`winspaces.exe` with no arguments starts the daemon (single instance enforced by the message window). Control flags:

| Flag | Behavior |
| :--- | :--- |
| `--exit` / `--kill` | Posts graceful shutdown to the running daemon; no-op if none |
| `--mission-control` / `-m` | Toggles Mission Control in the running daemon; no-op if none. Pinnable to the taskbar as a shortcut |
| `--dump [file]` | Diagnostic: writes all window metrics to `window_dump.txt` (or `file`) and exits |

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
  "mission_control": { "modifiers": 2, "vk": 38 },
  "switch_desktops": [ { "modifiers": 1, "vk": 49 }, ... ],
  "move_desktops":   [ { "modifiers": 3, "vk": 49 }, ... ],
  "prev":      { "modifiers": 1, "vk": 37 },
  "next":      { "modifiers": 1, "vk": 39 },
  "move_prev": { "modifiers": 13, "vk": 37 },
  "move_next": { "modifiers": 13, "vk": 39 },
  "workspace_rules": [
    {
      "name": "Brave (Work)",
      "aumid": "Brave.Profile1",
      "exe_path": "C:\\...\\brave.exe",
      "class_name": "Chrome_WidgetWin_1",
      "title_pattern": "",
      "display_index": 0,
      "desktop_index": 3,
      "show_cmd": 1,
      "rect": { "left": 0, "top": 0, "right": 1920, "bottom": 1040 },
      "is_snapped": false
    }
  ]
}
```

### Hotkey Encoding

`Hotkey` fields map directly onto `RegisterHotKey` parameters:
- `modifiers`: bitwise OR of `MOD_ALT = 0x1`, `MOD_CONTROL = 0x2`, `MOD_SHIFT = 0x4`, `MOD_WIN = 0x8`. Unknown bits are masked off on load.
- `vk`: Win32 virtual-key code (`0x31` = `1`, `0x25` = Left, `0x26` = Up, ...). `vk: 0` means unassigned; the hotkey is not registered.

### Workspace Rule Fields

- `aumid` / `exe_path` / `class_name` / `title_pattern`: window matchers scored per the fingerprint hierarchy in [`dwm.md`](dwm.md) §4 (AUMID is exact-match; title is one-directional contains).
- `display_index` / `desktop_index`: zero-based target monitor and space.
- `show_cmd`: `ShowWindow` command captured at snapshot time (`1` = normal, `3` = maximized).
- `rect`: target visible frame; for maximized rules it also seeds `rcNormalPosition` so un-maximizing lands on the right monitor.
- `is_snapped`: apply the DWM shadow-margin expansion + square-corner treatment from [`dwm.md`](dwm.md) §3.

### Normalization Contract (crash-proofing)

The daemon **never trusts the file shape**. `Config::normalize()` runs on every load:
- `switch_desktops` / `move_desktops` are resized to exactly `NUM_DESKTOPS` (4) entries, padding with unassigned hotkeys — hotkey registration indexes these lists directly and must not panic on a short array.
- Modifier bits outside the known mask are cleared.
- Unknown/missing optional fields fall back via serde defaults.

An **unparseable** file is renamed to `settings.json.bak` (never silently overwritten — it may hold captured workspace rules) and defaults are written in its place.

### C# Mirror Invariant

`gui/WinSpaces.Gui/Models/ConfigModel.cs` defaults must mirror `Config::default()` exactly (Alt+1..4, Ctrl+Alt+1..4, Alt+Left/Right, Alt+Shift+Win+arrows, Ctrl+Up), and `IpcService.Normalize()` performs the same 4-entry pad/truncate on load. Any change to defaults, field names, or serialization on one side must be applied to both — the JSON file is the contract.
