# Benchmark suite: plan

Status: delivered 2026-09-06 (`crates/winspaces-bench`, `dev bench`);
`docs/benchmarks.md` §3 is the authoritative reference from here on. Still
unexercised against a real daemon at delivery: the `--own` lifecycle
(`startup`, `ab`), `indicator_ab`, `tiling`, `soak`, and the `menu`
scenario's repaint path (needs `-Admin` while the daemon runs elevated).

## 1. Why

The daemon's budget is its main design constraint, and the only way to know
where it stands has been a PowerShell `Add-Type` recipe re-typed per session
(`docs/benchmarks.md` §3). Every measurement so far was ephemeral: numbers
were pasted into a doc, the harness was thrown away, and the next session
started from zero. The September refactor moved most of the code between
crates and modules; nothing says whether it cost anything.

Goal: a permanent, versioned suite that measures **everything measurable
without a human at the keyboard**, produces a machine-readable result plus a
readable report, and can diff two results so a regression or an improvement
is a table, not an opinion.

## 2. Ground rules carried over

From `docs/benchmarks.md` §1-§4; the suite encodes them so nobody has to
remember them:

1. Release build only for numbers (`opt-level = "z"`, LTO, the shipped
   profile). Debug is for smoke.
2. No regression claim without a same-session baseline: the `ab` command
   builds a pre-change commit in a worktree and measures both binaries
   back-to-back.
3. Name the state (cold / switching / MC-warm / fully warm). Every live
   scenario records the daemon state it started from and the floors before
   and after.
4. Leak checks cycle at least three times; one cycle cannot tell a cache
   from a leak.
5. Drive the daemon by posting messages only. Never `SendInput`, never
   `WM_CLOSE` to the tray menu (`WM_MENU_CLOSE = WM_APP + 41` instead).

## 3. Shape

One new workspace crate, **`winspaces-bench`** (a console bin, never
shipped), that depends on the four library crates and drives the daemon
binary. Zero new dependencies: `serde`/`serde_json` are already in the
workspace and `windows-sys` covers every counter the suite reads.

Rejected: `criterion` (~60 transitive crates compiled by `clippy
--all-targets` on every CI run; its reports cover only in-process code and
cannot express the daemon's handle and cycle counters) and `divan` (lighter,
but no machine-readable output, and the same coverage gap). Both are good
harnesses for a library; this project's cost lives in a running process, so
one JSON schema across micro, primitive, live and static groups matters more
than statistical plots.

```
crates/winspaces-bench/
  Cargo.toml
  src/main.rs          CLI dispatch (hand-parsed args, no clap)
  src/stats.rs         Summary { min, median, mean, p95, max, n }
  src/timing.rs        Runner: warm-up, calibration, samples, thread cycles
  src/report.rs        JSON model (schema 1), markdown renderer, load/save
  src/stamp.rs         machine stamp (CPU, MHz, OS build, monitors) and build stamp (git sha, exe)
  src/micro.rs + micro/*.rs        pure-logic benchmarks (group "micro")
  src/primitives.rs + primitives/*.rs   Win32 primitives the daemon pays per event (group "primitives")
  src/live.rs + live/*.rs          the daemon driver, sampler, scenarios, verdicts (group "live")
  src/static_info.rs   exe size, PE imports and sections, dependency count (group "static")
  src/compare.rs       diff two reports, flag regressions
scripts/bench.ps1      `dev bench` back end (PS 5.1, ASCII)
```

Results go to `.local/bench/results/<yyyymmdd-hhmmss>-<sha7>-<command>.json`
with a `.md` sibling; `.local/` is already git-ignored. Numbers that are
worth publishing are copied by hand into `docs/benchmarks.md` §5, as today.

## 4. Groups

### 4.1 `micro`: pure logic

Public API only (no `cfg(test)` builders, no feature flags: a `test-support`
feature would unify into the shipped binary under `cargo build --workspace`).
Each benchmark: warm-up, calibrate iterations to ~5 ms per sample, 30
samples, report ns/iter and thread cycles/iter (`QueryThreadCycleTime`),
min/median/mean/p95/max. `--smoke` runs one iteration of each.

| Name | What it prices |
| :--- | :--- |
| `tiling/compute_dwindle/n={1,2,4,9}` × gaps `{0,8/16}` | the layout per retile |
| `tiling/reconcile_order/9+1-1` | slot order per flush |
| `tiling/directional_neighbor/{left,right,up,down}` | focus/swap hotkey |
| `tiling/classify_drag/{ratio,reorder,snapback}` | drop of a tiled drag |
| `workspaces/score_rule/{hit,miss}` and `/rules=50` | rule matching per activation |
| `layout/same_layout/60/{equal,differ}` | the 5 s shadow diff |
| `layout/store/{parse,serialize}/8x60` | `layouts.json` round trip |
| `layout/store/upsert_at_cap` | topology eviction |
| `config/{parse,normalize,serialize}/rules=50` | `settings.json` reload |
| `i18n/{t_in,tf_in,tn_in}` | every painted string |
| `hotkey/to_string` | settings and menu labels |
| `hotkeys/decode` | every `WM_HOTKEY` |
| `topology/signature/3` | every reconcile |
| `indicator/round_rect_coverage/toast` | the toast's per-pixel alpha fill |

### 4.2 `primitives`: Win32 the daemon pays per event

In-process, read-only, no daemon needed, no input, no on-screen drawing.

| Name | What it prices |
| :--- | :--- |
| `win32/is_valid_window/{shell,app}` | the eligibility probe incl. the cross-process DWM cloak query |
| `win32/get_window_{title,class,aumid,exe}` | the identity queries the cache exists to avoid (`aumid` needs COM on the bench thread) |
| `win32/enum_valid_windows` | a full `EnumWindows` + eligibility walk (the scan floor) |
| `win32/stable_monitor_ids` | `QueryDisplayConfig`, per topology change |
| `gdi/paint_surface/462x689` | the menu's DIB allocate + alpha fixup, blitted to a memory DC |
| `io/config_save_atomic`, `io/layouts_save_atomic/8x60` | the persist path (temp file + rename) |

The "app" window is the first `is_valid_window` hit of `EnumWindows`; the
shell window is `GetShellWindow`. Both are probed, never touched.

### 4.3 `live`: the running daemon

Finds the daemon by `FindWindowW(WINSPACES_MSG_WINDOW_CLASS, WINSPACES_MSG_WINDOW_TITLE)`,
opens its process with `PROCESS_QUERY_LIMITED_INFORMATION`, and samples:

| Counter | API |
| :--- | :--- |
| cycles | `QueryProcessCycleTime` (delta per sample) |
| kernel / user time | `GetProcessTimes` (cross-check) |
| private bytes, working set, peak WS, page faults | `GetProcessMemoryInfo` (`PROCESS_MEMORY_COUNTERS_EX`) |
| GDI / USER objects | `GetGuiResources(0 / 1)` |
| handles | `GetProcessHandleCount` |
| threads | `CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD)` filtered by pid |
| write bytes | `GetProcessIoCounters` (optional; denied across integrity levels) |

Sampling is 1 Hz for phases, 20 Hz for in-paint peaks. Every phase records
the full series, not only the summary, so the JSON can be replotted later.

Scenarios (each phase names its state; `--quick` divides durations by 3):

| Scenario | Protocol | Verdicts |
| :--- | :--- | :--- |
| `idle` | A: 30 s floor | cycles/s summary, quiet-sample median |
| `switch` | A 30 s / B 6 events @ 10 s / C 30 s @ 400 ms / D 30 s settle, toggling monitor 0 between its current space and the next one, ending where it started | per-op cycles isolated and sustained; GDI/USER/handles/private return to floor |
| `mission_control` | 4 open/close cycles, 2 s dwell, 20 Hz sampling | retained after first open (cache) vs growth on cycles 2-4 (leak) |
| `menu` | 4 open/close cycles; while open, 20 forced repaints (`InvalidateRect` + `UpdateWindow`) and a 2 s async-invalidate loop with tight `GetGuiResources` polling | per-repaint cycles, in-paint GDI peak, every handle returned on close |
| `reload` | 10 × `WM_WINSPACES_RELOAD_CONFIG` @ 2 s | per-op cycles; hotkey re-registration leaves handles flat |
| `startup` (needs `--own`) | spawn the exe, poll for the message window at 1 ms, then sample at 1 s and 10 s | time-to-window, cold private bytes / GDI / USER / handles, first quiet tick |
| `indicator_ab` (needs `--allow-config-edit`) | the `switch` protocol twice, indicator off then on, config restored after | the A/B delta per switch |
| `tiling` (needs `--allow-disruptive`) | toggle on/off twice with a retile between | per-toggle cycles, floors |

Current space per monitor comes from the most recent topology in
`layouts.json` (`current_space`), so `switch` can end where it started; the
daemon has no query IPC and none is added.

Integrity: the daemon usually runs elevated. `WM_COMMAND` and the
`WM_WINSPACES_*` messages pass its `ChangeWindowMessageFilterEx` allow-list,
so `switch`, `mission_control`, `reload` and `--exit` work from a
medium-integrity harness. `WM_TRAYICON` and `WM_MENU_CLOSE` do not, so
`menu` is **skipped with a stated reason** when the harness is lower than
the daemon; `dev bench live --admin` elevates the harness. A skipped
scenario is recorded as such, never as a pass.

`--own`: stop the running daemon gracefully (`--exit`, wait for the exe
lock), start the given exe (default `target\release\winspaces.exe`), run,
`--exit` it, then `winspaces.exe --restart` so the user's posture (elevated
task or not) comes back. `ab <sha>` builds `<sha>` in a worktree under
`.local/bench/worktrees/` and runs `--own` twice, then `compare`.

### 4.4 `static`: the binary

Exe size (release and debug), PE section sizes, imported DLLs with import
counts (a 100-line PE reader; it is the "no runtimes" claim made checkable:
a `d3d11.dll` or `vcruntime` import would show up here), `Cargo.lock`
package count, the release profile knobs read from `Cargo.toml`.

## 5. Result and compare

Schema 1 (top level): `schema`, `generated_unix`, `command`, `machine`,
`build`, `groups.{micro,primitives,live,static}`. Micro/primitive entries:
`name`, `iters`, `samples`, `ns {min,median,mean,p95,max}`, `cycles {…}`.
Live scenarios: `name`, `state_before`, `phases[] {name, seconds, events,
samples[]}`, `metrics {…}`, `verdicts[] {check, pass, detail}`, `skipped`.

`compare base.json new.json` prints one table per group with base, new,
delta and a flag. Thresholds: micro/primitive median ns ±15 %; live floor
and per-op cycles ±20 % (the floor is noisy; the doc's own figure is ±60 %
between adjacent samples, so the per-op numbers are what to read); handle
floors exact (USER ±1 tolerated, documented); private bytes ±5 %; exe size
±2 %. A verdict that flipped from pass to fail is always flagged.

`report x.json` renders the markdown alone.

## 6. Integration

- `dev bench <micro|primitives|static|live|all|smoke|compare|report|ab>`
  through `scripts/bench.ps1`. Builds `-p winspaces-bench --release` (the
  daemon exe stays locked and untouched). `--admin` relaunches elevated.
- `dev check` gains `cargo run -p winspaces-bench -- smoke` (debug, one
  iteration each, no live group, a few seconds) so the suite cannot rot
  silently; CI runs the same script.
- `scripts/check-features.ps1` already enumerates `crates/*`, so the new
  crate is under the `windows-sys` feature audit from the first commit.
- Docs: `docs/benchmarks.md` §3 becomes "the toolkit is `dev bench`" (the
  hazards stay), §4 protocols reference the scenario names, §6 lists what
  the suite still cannot measure; `docs/crate-layout.md` §1-§2 add the
  crate; `AGENTS.md` architecture and build sections; `docs/README.md`;
  `CHANGELOG.md` Unreleased.

## 7. Work packages

Skeleton (types, CLI, stats, timing, report model, machine/build stamps)
is written first so the three packages compile against one API:

- **P1 micro + primitives**: `src/micro/*`, `src/primitives/*`, the
  registries, `--filter`, `--smoke`.
- **P2 live**: `src/live/*`: daemon discovery, elevation detection, sampler,
  driver, the eight scenarios, verdicts, `--own`, `--quick`, skips.
- **P3 static + compare + scripts + docs**: `src/static_info.rs`,
  `src/compare.rs`, markdown renderer polish, `scripts/bench.ps1`,
  `dev.ps1`, `cargo-tools.ps1` (`smoke` in `check`), the doc pages,
  `CHANGELOG.md`.

Then: review every package against the invariants above, `dev check`,
run `smoke`, `micro`, `primitives`, `static` and a non-disruptive `live
--quick --scenario idle,reload` against the user's daemon, fix, commit.

## 8. Acceptance

- `dev check` green, including the smoke run.
- `dev bench micro` and `dev bench primitives` finish in under two minutes
  on the reference machine and write JSON + markdown.
- `dev bench live --quick` runs `idle`, `switch`, `mission_control`,
  `reload` against the running daemon, skips `menu` with the UIPI reason
  when not elevated, and every non-skipped scenario carries verdicts.
- `dev bench compare` on two runs of the same binary flags nothing outside
  the documented noise, and flags a deliberate change (e.g. a doubled
  `SNAPSHOT_INTERVAL_MS` walk) when one is introduced.
- No number appears in any page other than `docs/benchmarks.md`.

## 9. Known limits

Unchanged from `docs/benchmarks.md` §6: DWM's share is invisible to process
counters; hover and real keystrokes need a human; the settings window is a
separate process (a `settings` scenario that spawns `--settings`, samples
it, and closes it with `WM_CLOSE` is a natural follow-up); a working-day
soak is a `--minutes N` idle run nobody has done yet.
