# Benchmarks

How to measure the daemon's cost, and what it measured last time.

Both halves matter. WinSpaces' budget is its main design constraint, but
performance numbers written into prose go stale silently — the previous
revision of [`tray-and-menu.md`](tray-and-menu.md) §5 sat unrevised long
enough to be off by ~8× on idle CPU, and nothing in the repo flagged it. This
page is the single place numbers live, dated and stamped with the machine they
came from. **Everywhere else should link here rather than repeat a figure.**

*Last verified: 2026-09-12, against 9f2a056.*

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
- **Overview-warm** — plus one Overview open. Still the big
  step: +8 GDI and ~+11 USER for the overlay window and its acrylic backdrop.
  (It used to be +13 GDI: the five overlay fonts were retained across close
  even though every show rebuilt them; they are now released on hide.)
- **Fully warm** — plus one tray menu open. The menu, by contrast, returns
  everything it takes.

Both retentions are genuine caches, not leaks — verified flat over repeated
cycles (§4.2). But it means a figure quoted without its state is unfalsifiable.
Real users reach "fully warm" within a minute of logging in, so **fully warm
is the honest number to quote for steady-state cost**; cold is only useful for
attributing where the cost comes from.

---

## 3. The Toolkit

`dev bench` (`crates/winspaces-bench`, back end `scripts/bench.ps1`) is the
toolkit: a versioned suite that drives the daemon the same way the retired
per-session PowerShell recipe did — by **posting messages**, never by
synthesizing input — and writes a machine-readable result plus this page's
kind of markdown report instead of a number someone has to remember to
paste in. §4's protocols below name the scenario that runs each one.

```powershell
.\dev bench <micro|primitives|static|live|all|smoke|compare|report|ab> [options]
```

| Command | What it runs |
| :--- | :--- |
| `micro` | pure-logic benchmarks: tiling, rule matching, layout, config, i18n |
| `primitives` | the Win32 calls the daemon pays per event (probes, GDI, IO) |
| `static` | the binary: exe size, PE sections and imports, `Cargo.lock` count |
| `live` | drives the running daemon and samples its counters, scenario by scenario |
| `all` | `micro` + `primitives` + `static` + `live`'s default scenario set |
| `smoke` | one iteration of `micro`/`primitives`, `static` best-effort, no `live` — what `dev check` runs so the suite cannot silently stop compiling |
| `compare <base.json> <new.json>` | diffs two results, one table per group, exits 2 if anything is flagged. A flag is a prompt to re-run both sides, not a verdict: sub-microsecond medians and the idle floor both move with boost state and ambient desktop activity |
| `report <result.json>` | re-renders a result's markdown, no new measurement |
| `ab <sha>` | builds `<sha>` in a throwaway worktree, runs `live --own` on it and on `HEAD` back to back, then `compare`s them; stops and restarts the running daemon twice |

`live` scenarios (`--scenario a,b,...`; default set is `idle,switch,
overview,menu,reload`): `idle`, `switch`, `overview`, `menu`,
`reload`, `startup` (needs `--own`), `indicator_ab` (needs
`--allow-config-edit`), `tiling` (needs `--allow-disruptive`), `soak`
(`--minutes N`). `--quick` divides every phase duration by 3.

Results land in
`.local\bench\results\<yyyymmdd-hhmmss>-<sha7>-<command>.json` with a `.md`
sibling; `.local\` is git-ignored. Numbers worth publishing are still copied
by hand into section 5 below.

`menu` needs a harness at least as elevated as the daemon (the UIPI note
under the hazards below) or it is skipped with that reason. Pass `-Admin` to
`dev bench` to relaunch the harness elevated for one run — an elevated
console closes on exit, so that mode writes to a fixed path under
`.local\bench\results\` and prints the report's first 40 lines back into the
window that asked for it.

For anyone still driving the daemon by hand — debugging the tool itself,
mostly — the message contract it automates is
[`ipc-and-config.md`](ipc-and-config.md), and the three hazards below remain
exactly as true as they ever were.

| Action | Message |
| :--- | :--- |
| Switch monitor `m` to space `d` | `WM_COMMAND` (0x0111), wparam `2000 + m*100 + d` — the `ID_TRAY_SWITCH_BASE` stride. `on_command` reads wparam whole, so a bare id works |
| Open the tray menu | `WM_TRAYICON` (0x0401), wparam 1, lparam `WM_RBUTTONUP` (0x0205) |
| **Close the tray menu** | **`WM_MENU_CLOSE` (0x8029 = `WM_APP + 41`) to the menu window** |
| Toggle Overview | `WM_USER + 103` (0x0467) |
| Reload config | `WM_USER + 100` (0x0464), after editing `settings.json` — flips a feature flag with no settings window involved |
| Force a repaint | `InvalidateRect(hwnd, NULL, FALSE)` + `UpdateWindow(hwnd)` |

Window classes: `WinSpacesMenu`, `WinSpacesOverview`,
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

`dev bench live --scenario switch` runs exactly this A/B/C/D protocol
against the running daemon; `indicator_ab` (needs `--allow-config-edit`)
runs the whole thing twice, indicator off then on, to price the toast by
this section's own rule rather than by subtraction.

### 4.2 Cache or leak

Growth alone proves nothing — fonts, window classes and thumbnail
registrations all allocate once. **Run at least three identical cycles.** A
cache steps once and holds flat; a leak keeps climbing. Both retaining
surfaces in this repo (Overview, the indicator's HWND) look alarming
after one cycle and are provably flat after four.

`dev bench live --scenario overview` runs four open/close cycles for
exactly this reason; its verdicts distinguish "retained after the first
open" from "still growing on cycle four".

### 4.3 Catching an in-paint peak

The menu's paint DIB exists only *inside* `WM_PAINT`, so a sample taken while
the menu merely sits open misses it entirely — you will read the resting value
and conclude the peak doesn't exist. Drive continuous **async** invalidation
(no `UpdateWindow`, so the daemon paints on its own thread) and poll
`GetGuiResources` in a tight loop alongside.

`dev bench live --scenario menu` drives four open/close cycles plus, while
each is open, 20 forced repaints and a 2 s async-invalidate loop with tight
`GetGuiResources` polling — this section's recipe, automated. It needs a
harness at least as elevated as the daemon (`-Admin`, §3) or it is
skipped with that reason.

### 4.4 What can't be driven remotely

**Hover.** The menu hit-tests against `GetCursorPos`, not the message's
`lparam`, so a posted `WM_MOUSEMOVE` changes nothing. A hover change
invalidates the *whole* window, so a forced full repaint reproduces the paint
cost exactly; only the hit test itself — a few rect comparisons — is omitted.

Anything genuinely requiring pointer position or real keystrokes needs a human
at the keyboard. Ask; do not inject input into the user's live session.

---

## 5. Latest Results

**2026-08-12, post-optimization** · AMD Ryzen 9 9900X @ 4.4 GHz · Windows 11
26100 · two monitors · release build · tray menu 462 × 689 px. Every A/B
below was measured back-to-back against a worktree build of `b6c0cea` (the
commit before the optimization series) in the same session; "baseline" means
that build, re-measured that day, not the previously published table.

### Resting cost, by state

| State | GDI | USER | Private | Idle CPU |
| :--- | ---: | ---: | ---: | :--- |
| Cold (fresh daemon) | 11 | 8 | 2.59 MB | — |
| + space switches | 11 | 10 | 2.64 MB | — |
| + Overview opened once | 19 | 21 | 3.09 MB | — |
| + tray menu opened once (**fully warm**) | 19 | 21 | 3.04 MB | 2.6 Mcycles/5 s avg; quiet samples **0.6–0.9 ≈ 0.003–0.005% of a core** |

Baseline fully warm, same session: 24 GDI / 21 USER / 3.15 MB, idle 7.4
Mcycles/5 s (5.0–12.4). The idle floor is no longer tick-dominated: the 5 s
shadow tick now walks the tracked set with cached per-window identity instead
of a full `EnumWindows` + COM/`OpenProcess` probe pass, so a quiet 5 s sample
is under 1 Mcycle and the average is set by ambient desktop activity (every
foreground change still does real work). USER counts wobble ±1 with ambient
shell state; average ≥10 samples, as ever.

### Transient — taken on open, returned on close

| Surface | GDI | USER | Private |
| :--- | ---: | ---: | ---: |
| Tray menu, open | +5 | +3 | ~0 |
| Tray menu, during `WM_PAINT` | +3 more | — | — |
| Space indicator, toast live | +2 | +1 | +0.09 MB |

All verified returning to the pre-open floor: the menu within its close, the
indicator within one second of the last toast (checked across 74 sustained
switches, with no staircase).

### Retained after first use — cache, not leak

| Surface | GDI | USER | Private | Verified |
| :--- | ---: | ---: | ---: | :--- |
| Overview | +8 | +11 | +0.26 MB settled | flat across 4 open/close cycles at 19 GDI / 21 USER |
| Space indicator | 0 | +1 | — | the reused HWND, by design |

Overview's retention is now only the overlay window and its DWM
backdrop: the five fonts it used to hold across close were rebuilt on every
show anyway and are released on hide since the optimization series.

### Per-operation CPU — same-session A/B vs baseline

| Operation | Baseline | Optimized |
| :--- | :--- | :--- |
| Tray menu, full-window repaint | 5.7 Mcycles ≈ 1.3 ms | 3.9 Mcycles ≈ 0.9 ms |
| Tray menu, hover repaint (two-row update rect) | = full window | **1.9 Mcycles ≈ 0.4 ms** |
| Space switch, indicator off | 21.7 isolated / 11.1 sustained | 17.6 isolated / 10.0 sustained |
| Space indicator toast (A/B delta) | +37.7 isolated / +12.5 sustained | **+14.6 isolated / +4.9 sustained** |

Hover changes now invalidate only the affected rows (`paint_surface_clipped`),
so a real hover repaint costs the two-row figure, not the full-window one. The
toast delta shrank because the fade timer parks for the whole 900 ms hold and
identical-alpha frames skip their `UpdateLayeredWindow`; the per-switch floor
came down via the in-memory activation fast path, the state-first scan
ordering, and the hidden-window skip in the switch sweep.

### What the optimization series changed (2026-08-12)

Relative to the same-day baseline measurements above: idle CPU −65% on the
average and −90% on quiet samples; toast cost −61%; hover repaint −67%
(−26% even for a forced full repaint); Overview retention −5 GDI.
Still true and deliberate: the Overview window/backdrop cache, the indicator's
reused HWND, and the double delivery of activation events through both hooks.

---

## 6. Not Measured

Stated so nobody mistakes silence for a clean result:

- **The settings window.** Separate process, so it costs the daemon nothing
  while closed; its own footprint has never been measured. `dev bench` has
  no scenario for it yet — spawning `--settings`, sampling it and closing it
  with `WM_CLOSE` is a natural follow-up.
- **DWM's share.** Acrylic blur, Mica, corner rounding and live thumbnails are
  composited in `dwm.exe` and never appear in the daemon's counters. The
  daemon-side numbers here are not the whole system cost of a surface.
- **Hover** (§4.4) and anything else requiring real pointer or keyboard
  input — `dev bench` drives the daemon by posting messages only, same as
  the recipe it replaces.
- **Startup and topology-change costs** — window scan, layout restore, RDP
  reconnect reconcile. The `startup` scenario (needs `--own`) covers the
  first of these; topology-change costs still have none.
- **Sustained real-world sessions.** A `soak` scenario exists
  (`--minutes N`, an extended `idle`), but nobody has pointed it at a whole
  working day yet. The longest continuous observation remains a few
  minutes; nothing here rules out slow growth beyond that.

## 7. Rejected Alternatives

**`criterion`** — around 60 transitive crates, compiled by `clippy
--all-targets` on every CI run for a tool that is never shipped, and its
reports cover only in-process code: it has no way to express the daemon's
handle and cycle counters that the `live` and `static` groups exist to read.

**`divan`** — lighter than `criterion`, but still no machine-readable output
and the same in-process-only coverage gap.

Either would be a fine choice for a library with no running-process cost to
account for. This project's cost lives in a daemon, not in the benchmark's
own process, so one JSON schema shared across `micro`, `primitives`, `live`
and `static` matters more here than either tool's statistical plots.

## See also

- [`tray-and-menu.md`](tray-and-menu.md) §5 for what keeps the menu cheap.
- [`overview.md`](overview.md) for the thumbnail cache the overlay keeps.
- [`space-indicator.md`](space-indicator.md) for the fade that this page prices.
