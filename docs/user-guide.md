# WinSpaces user guide

*Last verified: 2026-09-06, against b18bf56.*

How to install, use and troubleshoot WinSpaces. The architecture pages in
this folder are for contributors; this one is for the person running the app.

---

## 1. What it does

Every monitor gets its own set of spaces (1 to 9). Switching a space on one
display leaves the others exactly as they are; a window belongs to one space
of one display, unless you pin it to all of them. Windows on the spaces you
are not looking at are hidden without the application noticing: they keep
running, keep their position, and come back the instant you switch.

On top of that:

- **Mission Control** shows every space of a display and live thumbnails of
  its windows. Drag a thumbnail onto a space card to move the window there.
- **Taskbar follow**: clicking a taskbar button whose window lives on another
  space switches that display to it.
- **Tiling** (off by default) arranges the windows of a space automatically.
- **Workspace layouts**: capture where your windows are and let WinSpaces put
  them back on the next start, per monitor arrangement.

## 2. Installing

Run the installer. It installs per user under `%LOCALAPPDATA%\Programs\WinSpaces`,
asks for no administrator rights, and offers to start WinSpaces at login.
Running a newer installer over an installed copy upgrades it in place: the
running daemon is stopped gracefully first, so no window is left hidden.

- **Silent install**, for deployment scripts:
  `WinSpaces-Setup-x64-<version>.exe /VERYSILENT /NORESTART`. The post-install
  launch is skipped; start `winspaces.exe` yourself or log in again.
- **Portable mode**: copy `winspaces.exe` anywhere and put a `settings.json`
  next to it (an empty `{}` is enough). Settings, layouts and the log then
  live next to the exe instead of under `%LOCALAPPDATA%`.
- **Uninstall** stops the daemon, removes the autostart entry and the
  elevated task if one exists, and leaves `%LOCALAPPDATA%\WinSpaces` in
  place so a reinstall finds your configuration. Delete that folder by hand
  if you want a clean slate.

## 3. First steps

WinSpaces lives in the notification area. Left-click the icon to open
Mission Control; right-click it for the menu (Settings, capture and restore
of the workspace layout, taskbar mode, updates, exit). The badge shows the
current space number of each display.

Every default shortcut is listed in §9; the ones to learn first:

| Do | Press |
| :--- | :--- |
| Open Mission Control | `Win+Tab`, `Ctrl+Up`, or click the tray icon |
| Go to space N on the display with the focused window | `Alt+N` |
| Take the focused window to space N and follow it | `Ctrl+Alt+N` |
| Previous / next space | `Alt+Left` / `Alt+Right` |
| Pin the focused window to every space of its display | `Ctrl+Alt+Shift+P` |

Every shortcut can be changed in Settings, on the *Hotkeys* page. A "Space N"
badge flashes near the taskbar of the display that just switched.

In Mission Control: click a space card to switch to it, drag a card to
reorder spaces, use the "+" tile (or drop a window on it) for a new space,
and the × on a card to remove one (its windows move to the neighbouring
space). Press `1`-`9` to switch, `P` over a window card to pin it, `Esc` to
close.

**Taskbar mode.** By default a hidden window also disappears from the taskbar
and `Alt+Tab`, like a window on another Windows virtual desktop.
`Ctrl+Alt+Shift+S` (or the tray menu) switches to a mode where every window
keeps its taskbar button; clicking a button then takes you to its space.

## 4. Settings and files

Open Settings from the tray menu or with `winspaces.exe --settings`. The
window runs as its own process, so nothing you do there can stall the daemon.
Changes apply immediately. Pages:

- **System**: language, theme, autostart, the space indicator, `Win+Tab`
  interception, administrator mode (§6.2), export and import of settings.
- **Tiling**: enable, gaps, and the float rules that keep specific apps out
  of the layout.
- **Hotkeys**: every binding, recorded by pressing the keys.
- **Workspaces**: capture the current layout, restore it, and the rules the
  capture produced.

Everything is plain JSON under `%LOCALAPPDATA%\WinSpaces\` (or next to the
exe in portable mode):

| File | Holds | Notes |
| :--- | :--- | :--- |
| `settings.json` | every setting, hotkey and rule | Safe to edit by hand while the daemon runs; use the tray or `--restart` to reload |
| `layouts.json` | one window layout per monitor arrangement | Written by the daemon; you rarely touch it |
| `winspaces.log` | what the daemon did, with timestamps | Rotated at 5 MB to `winspaces.log.old` |

A file that no longer parses is moved aside as `settings.json.bak` or
`layouts.json.bak` and replaced by defaults, so a typo never costs you your
configuration: fix the `.bak` and rename it back.

## 5. Workspace layouts

*Capture Workspace Layout* (tray or Settings) records every open window's
app, display, space and position as a rule. With *Auto-Restore Spaces Layout
on Launch* enabled, the daemon puts matching windows back when it starts;
*Restore Workspace Layout* does the same on demand. Rules only ever touch
windows that are already open: a window you open later appears on the space
you are on, whatever its app's rule says, so launching an app on an empty
space never pulls you to another one.

Independently of the rules, the daemon remembers where windows were for each
monitor arrangement it has seen. Unplug a display, or connect over Remote
Desktop, and the windows are reflowed by Windows onto what is left; plug it
back in, or disconnect the session, and WinSpaces moves them back to the
monitor and space they had. It waits a moment after the change and then holds
the placement for a few seconds while Windows itself finishes reshuffling.

## 6. Tiling

`Ctrl+Alt+Shift+T` turns tiling on and off for every display. With it on,
the windows of a space fill the work area in a spiral of halves, with the
gaps you set. Useful habits:

- `Ctrl+Alt+Shift+F` floats the focused window out of the layout (and puts
  it back). To keep an app out of tiling permanently, add a float rule.
- **Fullscreen is maximize.** Maximize a tile (button, `Win+Up`, drag to the
  top edge) and it covers the layout; restore it and it returns to its slot.
- Drag a tile onto another to swap them; drag the border between the two
  first tiles to change their ratio; hold `Shift` while dragging to flip the
  split between side-by-side and stacked.
- A window that refuses its slot (a minimum size larger than the tile, an
  administrator window) is floated automatically after a few tries and gets
  a normal frame again; it rejoins the layout when the space gets more room.

Float rules are listed on the Tiling settings page; new ones are added by
editing `settings.json` under `tiling.float_rules` for now.

## 7. Troubleshooting

### 7.1 My windows disappeared

If WinSpaces was killed or crashed while windows were hidden, those windows
stay hidden: they still run and still show in Task Manager. Start WinSpaces
again and it reclaims them on startup. If that is not possible, run the
recovery script from the install folder in PowerShell:

```powershell
& "$env:LOCALAPPDATA\Programs\WinSpaces\scripts\recover-windows.ps1"
```

It stops a running daemon first (deliberately: recovering underneath a live
daemon leaves it unable to hide or show anything), then un-hides every
window WinSpaces was tracking. Start WinSpaces again afterwards. If the
daemon was running in administrator mode, run PowerShell as administrator
too, or the script cannot reach it.

### 7.2 Administrator windows are not managed

Windows does not let a normal process hide, move or read keys from an
elevated window, so an elevated app stays visible on every space and
`Win+Tab` interception pauses while it has focus. To manage such windows,
turn on *Run with Administrator Privileges* on the System page (or run
`winspaces.exe --enable-elevation` from an administrator prompt). One UAC
prompt registers a logon task; from then on WinSpaces starts elevated at
login without prompting. The trade-off is that the daemon itself runs with
administrator rights. Turn it off the same way.

### 7.3 A shortcut does nothing

WinSpaces registers all its hotkeys together, and Windows refuses a key
another application already owns; when that happens none of them are
registered. Look for `[FAIL] hotkey` in the log to see which one collided,
then change it on the Hotkeys page or free it in the other app.

### 7.4 `Win+Tab` still opens Task View

Either the *Intercept Win+Tab* option is off, or an administrator window
has focus (§7.2). `Ctrl+Up` and the tray icon open Mission Control
regardless.

### 7.5 A window shows on every space

It is pinned. Unpin it with `Ctrl+Alt+Shift+P` while it has focus, or with
the pin button on its card in Mission Control. Pins are remembered with the
workspace layout.

### 7.6 Monitors switch off and on and windows land on the wrong screen

WinSpaces restores the layout of a known monitor arrangement a moment after
it comes back and then enforces it for a while against Windows' own
reshuffle, so give it a few seconds. A DisplayPort monitor that is powered
off usually counts as unplugged, an HDMI one usually does not; the layout is
remembered per arrangement either way, so both states get their own.

### 7.7 Getting help

Attach `winspaces.log` when reporting a problem. For more detail, start the
daemon with the environment variable `WINSPACES_LOG=debug`.
`winspaces.exe --dump` writes every window's metrics to `window_dump.txt`,
and `winspaces.exe --elevation-status` prints how the daemon and the
elevated task are set up.

## 8. Command line

| Command | Effect |
| :--- | :--- |
| `winspaces.exe` | Start the daemon (a second copy exits at once) |
| `winspaces.exe --settings` | Open the settings window |
| `winspaces.exe --mission-control` (`-m`) | Toggle Mission Control; pin this as a taskbar shortcut |
| `winspaces.exe --tiling-toggle` (`-t`) | Turn tiling on or off |
| `winspaces.exe --restart` (`-r`) | Stop the daemon and start it again |
| `winspaces.exe --exit` | Stop the daemon, un-hiding every window first |
| `winspaces.exe --enable-elevation` / `--disable-elevation` / `--elevation-status` | Administrator mode (§7.2) |
| `winspaces.exe --dump [file]` | Write every window's metrics to a file |

The full reference, including the IPC messages behind these flags, is
[`ipc-and-config.md`](ipc-and-config.md) §3.

## 9. Hotkey reference

Every binding can be changed on the *Hotkeys* page of Settings.

| Action | Shortcut / Trigger |
| :--- | :--- |
| Toggle Mission Control | `Win+Tab` / `Ctrl+Up` / tray icon click |
| Switch to space 1..9 | `Alt+1..9` (or press `1..9` in Mission Control) |
| Move window to space 1..9 and follow | `Ctrl+Alt+1..9` (or drag the window onto a space card) |
| New space | Mission Control "+" tile (or drop a window on it) / tray submenu |
| Remove space | × on a hovered space card in Mission Control / tray submenu |
| Previous / next space | `Alt+Left` / `Alt+Right` |
| Move window to previous / next space and follow | `Alt+Shift+Win+Left` / `Alt+Shift+Win+Right` |
| Pin window to every space (sticky) | `Ctrl+Alt+Shift+P` (or the pin button / `P` on a hovered card in Mission Control) |
| Toggle taskbar mode | `Ctrl+Alt+Shift+S` |
| Toggle dynamic tiling | `Ctrl+Alt+Shift+T` |
| Focus left / right / up / down | `Ctrl+Alt+Shift+Left` / `Right` / `Up` / `Down` |
| Swap left / right / up / down | `Ctrl+Shift+Win+Left` / `Right` / `Up` / `Down` |
| Shrink / grow split ratio | `Ctrl+Alt+Shift+-` / `+` |
| Toggle float on the focused window | `Ctrl+Alt+Shift+F` |
| Toggle split orientation | `Ctrl+Alt+Shift+O` (or `Shift` + drag a tiled window) |
| Fullscreen a tile | Maximize it (button, `Win+Up`, or drag to the top edge); restore to return it to its tile |
| Exit WinSpaces | `Ctrl+Alt+Shift+Q` |

## See also

- [`README.md`](../README.md) for the feature overview and how to build from source.
- [`ipc-and-config.md`](ipc-and-config.md) for the settings and layouts file schemas.
- [`tiling.md`](tiling.md) for how the tiler decides what it does.
- [`display-topology.md`](display-topology.md) for what happens on monitor and RDP changes.
