# Benchmarks

How to measure the daemon's cost, and what it measured last time.

Both halves matter. WinSpaces' budget is its main design constraint, but
performance numbers written into prose go stale silently — the previous
revision of [`tray-and-menu.md`](tray-and-menu.md) §5 sat unrevised long
enough to be off by ~8× on idle CPU, and nothing in the repo flagged it. This
page is the single place numbers live, dated and stamped with the machine they
came from. **Everywhere else should link here rather than repeat a figure.**

---

## 1. Ground Rules

Five things that will produce wrong numbers if you skip them:

1. **Measure a release build.** Debug idles at roughly double, and the gap is
   not uniform across operations.
2. **Never quote a number as a regression without a floor from the same
   machine and session.** Build the pre-change commit in a throwaway worktree
   (`git worktree add <tmp> <sha>`), run *that* binary as the live daemon, and
   measure it back-to-back with the new one. Background load, CPU boost state
   and monitor topology all move these figures more than most code changes do.
3. **The user must be idle.** The daemon holds a foreground hook and a shell
   hook; every window activation, even a background app stealing focus, does
   real work that lands in the sample.
4. **Name the state** (§2). "Idle" alone is meaningless.
5. **Cycle anything you suspect of leaking at least three times** (§4.2). One
   open/close cycle cannot tell a cache from a leak, and both of this repo's
   heavyweight surfaces happen to be caches.

Instruments, all cheap and non-invasive:

| Quantity | API | Notes |
| :--- | :--- | :--- |
| CPU | `QueryProcessCycleTime` | Actual cycles retired by the process. Divide by (seconds × clock) for "% of a core". Prefer this over wall time — it excludes time the process was descheduled. |
| GDI / USER objects | `GetGuiResources(h, 0 / 1)` | Needs a handle from `OpenProcess(0x1000, ...)` — `PROCESS_QUERY_LIMITED_INFORMATION`. |
| Memory | `Process.PrivateMemorySize64` | Private working set is noisier; private commit is the stable figure. |

---

## 2. Naming the State

The single biggest source of confusion in past measurements. A daemon's
resting cost depends entirely on **which surfaces it has been asked to open at
least once**, because two of them allocate on first use and keep what they
allocate:

- **Cold** — freshly started, nothing exercised.
- **Switching** — cold plus some space switches. The indicator retains one
  HWND after its first toast.
- **Mission-Control-warm** — plus one Mission Control open. This is the big
  step: +13 GDI and +12 USER, and **closing it returns none of them.**
- **Fully warm** — plus one tray menu open. The menu, by contrast, returns
  everything it takes.

Both retentions are genuine caches, not leaks — verified flat over repeated
cycles (§4.2). But it means a figure quoted without its state is unfalsifiable.
Real users reach "fully warm" within a minute of logging in, so **fully warm
is the honest number to quote for steady-state cost**; cold is only useful for
attributing where the cost comes from.

---

## 3. The Toolkit

Everything below drives the daemon by **posting messages**, never by
synthesizing input. Same code paths as real interaction, no `SendInput`, no
focus theft beyond what the daemon itself does. See
[`ipc-and-config.md`](ipc-and-config.md) for the message contract.

```powershell
Add-Type @'
using System;
using System.Runtime.InteropServices;
public class B {
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern IntPtr FindWindowW(string c, string w);
  [DllImport("user32.dll")] public static extern bool PostMessageW(IntPtr h, uint m, UIntPtr wp, IntPtr lp);
  [DllImport("user32.dll")] public static extern bool InvalidateRect(IntPtr h, IntPtr r, bool erase);
  [DllImport("user32.dll")] public static extern bool UpdateWindow(IntPtr h);
  [DllImport("user32.dll")] public static extern uint GetGuiResources(IntPtr h, uint flags);
  [DllImport("user32.dll")] public static extern bool SetProcessDPIAware();
  [DllImport("kernel32.dll")] public static extern bool QueryProcessCycleTime(IntPtr h, out ulong c);
  [DllImport("kernel32.dll")] public static extern IntPtr OpenProcess(uint a, bool inh, uint pid);
}
'@
[void][B]::SetProcessDPIAware()   # the daemon is per-monitor-DPI-aware; pwsh is not
$msg = [B]::FindWindowW('WinSpacesMessageClass','WinSpacesMessageWindow')
$ph  = [B]::OpenProcess(0x1000, $false, (Get-Process winspaces).Id)
```

| Action | Message |
| :--- | :--- |
| Switch monitor `m` to space `d` | `WM_COMMAND` (0x0111), wparam `2000 + m*100 + d` — the `ID_TRAY_SWITCH_BASE` stride. `on_command` reads wparam whole, so a bare id works |
| Open the tray menu | `WM_TRAYICON` (0x0401), wparam 1, lparam `WM_RBUTTONUP` (0x0205) |
| **Close the tray menu** | **`WM_MENU_CLOSE` (0x8029 = `WM_APP + 41`) to the menu window** |
| Toggle Mission Control | `WM_USER + 103` (0x0467) |
| Reload config | `WM_USER + 100` (0x0464), after editing `settings.json` — flips a feature flag with no settings window involved |
| Force a repaint | `InvalidateRect(hwnd, NULL, FALSE)` + `UpdateWindow(hwnd)` |

Window classes: `WinSpacesMenu`, `WinSpacesMissionControl`,
`WinSpacesSettingsClass` (title `WinSpaces Settings`).

### Three hazards, each of which has actually bitten

> **Never close the tray menu with `WM_CLOSE`.** `DefWindowProc` turns it into
> `DestroyWindow`, so the window dies without `close_menu()` ever running —
> and that is the only code that unhooks `WH_MOUSE_LL` and clears
> `MENU_STATE`. The result is two stranded global input hooks: the daemon's
> `WH_KEYBOARD_LL` swallows **Space**, arrows, Enter and Esc system-wide for as
> long as `is_menu_open()` lies, and the orphaned mouse hook eats button-downs.
> `IsWindow` returns false afterwards, so it looks like it worked. Recovery is
> to kill the daemon; hooks die with the process. `menu_wnd_proc` now carries a
> `WM_DESTROY` guard that catches this, but post `WM_MENU_CLOSE` anyway.

- **`FindWindowW` needs both class *and* title.** PowerShell coerces `$null`
  to `""`, which means "match only untitled windows" — so a null title
  silently returns 0 for the message window and the settings window. Titleless
  windows (menu, overlay) appear to work, which makes the trap worse.
- **The exe is locked while the daemon runs.** `--exit` before rebuilding, or
  the build fails with `Access is denied` and you carry on measuring the old
  binary without noticing.

---

## 4. Protocols

### 4.1 The cost of one feature: A/B, not subtraction

To price a feature that rides on an existing operation — a toast that only
appears *because* a space switch happened — **run the whole protocol twice,
once with the feature disabled and once enabled, driving identical inputs**,
and diff. Subtracting an idle floor from a single run cannot separate the two,
and the floor is noisy enough (±60% between adjacent samples) to swamp the
answer outright.

Four phases, sampled at 1 Hz:

| Phase | Duration | Drive | Isolates |
| :--- | :--- | :--- | :--- |
| A — floor | 30 s | nothing | idle, including the 5 s shadow tick |
| B — isolated | 60 s | one event per 10 s | cost of one complete, non-overlapping operation |
| C — sustained | 30 s | one event per 400 ms | worst case, and any re-entrant path |
| D — settle | 30 s | nothing | **the leak check** — handles must return to A |

Phase D is the one that matters. A peak is fine; a floor that moved is not.

### 4.2 Cache or leak

Growth alone proves nothing — fonts, window classes and thumbnail
registrations all allocate once. **Run at least three identical cycles.** A
cache steps once and holds flat; a leak keeps climbing. Both retaining
surfaces in this repo (Mission Control, the indicator's HWND) look alarming
after one cycle and are provably flat after four.

### 4.3 Catching an in-paint peak

The menu's paint DIB exists only *inside* `WM_PAINT`, so a sample taken while
the menu merely sits open misses it entirely — you will read the resting value
and conclude the peak doesn't exist. Drive continuous **async** invalidation
(no `UpdateWindow`, so the daemon paints on its own thread) and poll
`GetGuiResources` in a tight loop alongside.

### 4.4 What can't be driven remotely

**Hover.** The menu hit-tests against `GetCursorPos`, not the message's
`lparam`, so a posted `WM_MOUSEMOVE` changes nothing. A hover change
invalidates the *whole* window, so a forced full repaint reproduces the paint
cost exactly; only the hit test itself — a few rect comparisons — is omitted.

Anything genuinely requiring pointer position or real keystrokes needs a human
at the keyboard. Ask; do not inject input into the user's live session.

---

## 5. Latest Results

**2026-08-12** · AMD Ryzen 9 9900X @ 4.4 GHz · Windows 11 26100 · two monitors
· release build · tray menu 462 × 689 px

### Resting cost, by state

| State | GDI | USER | Private | Idle CPU |
| :--- | ---: | ---: | ---: | :--- |
| Cold (fresh daemon) | 11 | 8 | 2.64 MB | — |
| + space switches | 11 | 9 | 2.76 MB | 6.9 Mcycles/5 s ≈ **0.03% of a core** (5.4–10.3) |
| + Mission Control opened once | 24 | 21 | 2.91 MB | — |
| + tray menu opened once (**fully warm**) | 24 | 21 | 3.16 MB | 8.5 Mcycles/5 s ≈ **0.04% of a core** (5.8–12.4) |

Idle CPU is dominated by the 5 s shadow/snapshot tick, which is why adjacent
5 s samples vary by 2×. Average over ≥10 samples or the number is meaningless.

### Transient — taken on open, returned on close

| Surface | GDI | USER | Private |
| :--- | ---: | ---: | ---: |
| Tray menu, open | +4 | +3 | +0.05 MB |
| Tray menu, during `WM_PAINT` | +4 more | — | — |
| Space indicator, toast live | +2 | +1 | +0.25 MB peak |

All verified returning to the pre-open floor: the menu within its close, the
indicator within one second of the last toast (checked across 79 back-to-back
switches, with no staircase).

### Retained after first use — cache, not leak

| Surface | GDI | USER | Private | Verified |
| :--- | ---: | ---: | ---: | :--- |
| Mission Control | +13 | +12 | +0.15 MB | flat across 4 open/close cycles |
| Space indicator | 0 | +1 | — | the reused HWND, by design |

### Per-operation CPU

| Operation | Cost |
| :--- | :--- |
| Tray menu, full repaint (= one hover change) | ~5.2 Mcycles ≈ **1.2 ms** |
| Space switch, indicator off | 10.5 Mcycles sustained / 16.3 isolated |
| Space indicator toast (A/B delta) | **+35 Mcycles** isolated ≈ 8 ms spread over its 1.27 s life ≈ 0.6% of a core while visible; **+13 Mcycles** under sustained switching, where overlapping toasts never complete their fade |

### Drift since the previous published figures

The old §5 table claimed 2.4 MB idle, 0.8 Mcycles/5 s, and a 3.5 Mcycle
repaint. Memory has barely moved (2.4 → 2.64 MB cold). **CPU has not**: idle is
~8× and a repaint ~1.5× the published figure. Confirmed against a build of the
pre-`space-indicator` commit that this predates that feature — it is
accumulated cost from everything since the table was written (the topology
shadow tick; dynamic per-monitor spaces, which also lengthen the menu, and
repaint cost scales with window area). Not bisected further.

---

## 6. Not Measured

Stated so nobody mistakes silence for a clean result:

- **The settings window.** Separate process, so it costs the daemon nothing
  while closed; its own footprint has never been measured.
- **DWM's share.** Acrylic blur, Mica, corner rounding and live thumbnails are
  composited in `dwm.exe` and never appear in the daemon's counters. The
  daemon-side numbers here are not the whole system cost of a surface.
- **Anything requiring real pointer or keyboard input** (§4.4).
- **Startup and topology-change costs** — window scan, layout restore, RDP
  reconnect reconcile.
- **Sustained real-world sessions.** The longest continuous observation is a
  few minutes. Nothing here rules out slow growth over a working day.
