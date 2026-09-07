# Display Topology, RDP, and Layout Restore

How WinSpaces survives the display set changing underneath it — docking, a
monitor sleeping, and above all a Remote Desktop session.

*Last verified: 2026-09-06, against b18bf56.*

---

## 1. Why this is hard

**RDP does not add a display, it replaces the topology.** On connect, Windows
detaches every physical display and attaches the RDP Indirect Display Driver
(`SWD\REMOTEDISPLAYENUM\RDPIDD_INDIRECTDISPLAY&SESSIONID_xxxx`) as the sole
monitor. The OS then reflows every top-level window into that one display, and
on disconnect it does not put them back. Nothing about this requires WinSpaces
to be running — it is the baseline behavior the daemon has to repair.

Three properties of the Win32 surface make naive handling actively destructive:

- **`HMONITOR` is not stable.** Handles are reissued across a topology change.
  Comparing a cached handle to a fresh `MonitorFromWindow` result silently
  fails.
- **`\\.\DISPLAYn` is a *slot*, not a monitor.** Windows recycles slot names by
  attach order, so the RDP virtual display can inherit the name a physical
  panel held moments earlier. Keying per-monitor state on `szDevice` grafts one
  monitor's spaces onto another's.
- **`WM_DISPLAYCHANGE` arrives late and in bursts.** It fires *after* the OS has
  reflowed windows, several times per transition. Capturing a layout at that
  moment records the damage, not the layout worth restoring.

## 2. Stable monitor identity

`topology::stable_monitor_ids()` (`crates/winspaces-core/src/topology.rs`)
walks `QueryDisplayConfig(QDC_ONLY_ACTIVE_PATHS)` and, per active path, calls
`DisplayConfigGetDeviceInfo` twice:

| Request | Yields |
| :--- | :--- |
| `GET_SOURCE_NAME` | `viewGdiDeviceName` — `\\.\DISPLAY2` |
| `GET_TARGET_NAME` | `monitorDevicePath` — `\\?\DISPLAY#BNQ805B#5&1f33c64f&0&UID4354#{...}` |

The device path embeds the EDID manufacturer/product code and the physical
connector instance, so it survives reordering, docking and RDP. The RDP IDD gets
its own distinct path and can never be mistaken for a physical panel. When a
target reports a blank path the EDID ids plus connector instance are used
instead; when the whole query fails the GDI device name is the fallback, which
is exactly the pre-existing behavior.

`MonitorState.stable_id` carries this; `MonitorState.device` is kept for logging
only. `SpaceManager::handle_display_change` re-associates per-monitor space
state on `stable_id`.

**Topology signature** = the sorted, `|`-joined stable ids of the attached set
(`topology::signature_from_ids`). Sorting makes it independent of enumeration
order, so the same physical desk always produces the same key.

## 3. Never claim monitor 0

`SpaceManager::monitor_index_for_hwnd` resolves a window to a monitor by
`HMONITOR` identity first, then by locating the window's centre inside a
monitor's `rcMonitor`.

It returns `Option`, and that is the point. Both call sites previously ended in
`.unwrap_or(0)`, so any stale-handle miss re-homed the window onto the primary
display **and onto that display's current space**, destroying the space
assignment. It fired without any display change at all — logs showed entire
window sets flip-flopping between monitors 30 seconds apart. Callers now leave
the window's tracking untouched when it cannot be resolved.

`scan_untracked_windows` additionally skips its cross-monitor re-home branch in
two transient states:

- while `SpaceManager.reconcile_pending` is set — mid-burst the OS is still
  moving windows, so any conclusion drawn then is wrong;
- while `is_settling()` — for `SETTLE_MS` (4 s) after *any* bulk placement.

The second guard is not theoretical. `SetWindowPlacement`/`SetWindowPos` do not
take effect synchronously: a scan **90 ms** after a restore was observed reading
the pre-move rect, deciding two windows had "moved across displays", and
re-homing them onto that monitor's current space — undoing the restore and
collapsing their space assignment. `begin_settle` is called at the end of both
`layout_store::restore_snapshot` and `restore_workspace_rules`.

Skipping the re-home is not enough on its own, because outside agents also
move windows *after* the settle expires. Windows' "remember window locations
based on monitor connection" sweep (and some apps' own display-change
handlers) reposition windows **~10 s** after a monitor returns; observed live,
it moved a restored Brave window onto the other monitor, a scan then adopted
that as a user drag, and the next shadow save wrote the wrong layout over the
good one. So for `RESTORE_ENFORCE_MS` (15 s) after a topology restore its
placements are **enforced**: the scan's cross-monitor branch pushes a drifted
restore target back (`try_enforce_restore`) instead of re-homing it, a
one-shot `TIMER_RESTORE_VERIFY` sweep at 12 s catches drift even when no scan
runs, and the shadow tick treats the whole window as mid-transition so a
capture can never save the drift the enforcement is about to undo.

The window is **sliding**, not fixed: each successful push-back (or heal, see
below) restarts the 15 s clock, capped at four times `RESTORE_ENFORCE_MS`
(60 s) from the restore. Observed live with a fixed window: the sweep was
still fighting at expiry and re-moved the window ~450 ms after the deadline,
so the very next scan adopted the drift. A push-back is proof the fight is
still on; silence for a full 15 s is the actual signal that it is over. A
genuine user drag inside the window is fought only until the cap — accepted,
because monitor reconnects are rare and nobody re-arranges windows in the
first minute of one.

Cloaked windows need a third mechanism, because the sweep moves them too and
both of the above are blind to hidden windows (the scan returns early on
`HIDDEN_MASK`; the verify pass skips them since geometry does not reliably
stick to a cloaked window). Observed live: a Brave window restored onto a
background space drifted while cloaked, kept its correct tracking, and then
*surfaced on the wrong monitor* when its space was next shown — the uncloak
does not re-assert geometry. So `switch_space`'s show loop heals as it
reveals: a shown window whose tracking still matches its restore target but
whose rect sits on another monitor gets the restored placement re-applied
(`heal_restored_placement`). No time window applies — a cloaked window cannot
have been user-dragged, and the scan at the top of the same `switch_space`
call re-homes genuine drags before the show loop runs, so a stale target
simply stops matching. Targets are pruned with dead handles in
`prune_dead_windows` so a recycled HWND can never inherit one.

The tick comparison is wrapping-safe (`tick_before`): `GetTickCount` rolls over
every ~49 days, and a zero deadline must mean "unset" rather than "expired",
otherwise any machine up longer than 24.8 days reads as permanently settling.

## 4. Shadow, persist, restore

This tick-driven orchestration (`reconcile_topology`, `shadow_tick`,
`persist_shadow`) lives in the bin, not in `winspaces-core` — deliberately.
`layout_store` and `workspaces` supply the pure mechanics (snapshot shape,
scoring, placement math), but the tick itself mutates `AppState`'s `shadow`,
`layouts` and `last_signature` fields together, and `AppState` is private to
the bin by design (see [`crate-layout.md`](crate-layout.md)). Splitting the
orchestration out would mean handing `winspaces-core` a `&mut AppState` it has
no business seeing, so it stays a bin-level concern that calls down into
`winspaces-core`'s pure halves.

Because `WM_DISPLAYCHANGE` is too late to capture anything useful, the daemon
keeps a **shadow** of the live layout, refreshed every `SNAPSHOT_INTERVAL_MS`
(5 s) whenever the topology is healthy, no reconcile is pending, and no restore
is settling. Changes mark it dirty and arm a `PERSIST_DEBOUNCE_MS` (5 s) write;
it is also flushed on clean exit and on `WM_ENDSESSION`. The debounce is short
on purpose — a restart inside the window replays a stale snapshot and reverts
spaces the user has since rearranged.

Capture sorts windows into a canonical order (monitor, space, exe, class,
origin). `EnumWindows` returns z-order, so without it merely focusing a
different window reorders the list, every tick compares unequal, and the file is
rewritten forever. For the same reason `same_layout` ignores each window's
`name`: it embeds the live title, which changes on every browser tab switch.

`layouts.json` (sibling of `settings.json`, atomic temp-file + rename) holds one
`TopologySnapshot` per signature, capped at 8 with least-recently-captured
eviction. Kept separate from `settings.json` on purpose: layout writes are
frequent, and a torn write must never cost the user their hotkeys or rules.

Each `MonitorSnapshot` also records the monitor's `space_count`. Counts are
structural rather than layout: they are applied from the stored snapshot at
startup and on reconcile even when auto-restore is off, and a user-initiated
add/remove writes them straight into the stored topology entry
(`persist_space_counts`) because the shadow tick refuses empty-window captures
and would otherwise never persist a count change made with nothing open.

Each `WindowSnapshot` stores geometry twice — the exact physical-pixel `rect`
for a pixel-perfect replay onto an unchanged monitor, and `rel`, the same rect
as fractions of the work area, for a monitor that came back at a different
resolution or scale. `resolve_rect` picks the former when work area *and* DPI
match, the latter otherwise, and clamps the result into `rcWork` either way.

### Reconcile

`WM_DISPLAYCHANGE` and `WM_WTSSESSION_CHANGE` (console/remote connect and
disconnect, via `WTSRegisterSessionNotification`) both set `reconcile_pending`
and restart a single `RECONCILE_DEBOUNCE_MS` (1200 ms) timer. When it fires,
`reconcile_topology`:

1. Runs `handle_display_change()` to rebuild the monitor table.
2. Computes the signature; returns early if unchanged.
3. If `GetSystemMetrics(SM_REMOTESESSION)`, pauses shadowing and stops — the
   layout in a remote session is disposable.
4. Otherwise replays the stored snapshot for that signature, if one exists.

The remote-session check is belt and braces: because the RDP display carries its
own stable id, the remote topology gets its own signature and **cannot overwrite
the desk layout on disk** even without it.

### Replay

`layout_store::restore_snapshot` un-hides every tracked window first (geometry
does not stick to a hidden window, whichever backend — DWM cloak, shell cloak,
or forced minimize — hid it), then assigns live windows to snapshot entries in
two passes:

1. **Exact handles.** Each snapshot entry records the HWND (plus pid and exe)
   of the window it was captured from. Within the session that captured it,
   the handle *is* the window's identity — no OS mechanism identifies a window
   more reliably, and nothing else can tell apart twin windows of one app
   (two default-profile Brave windows) whose titles have changed since
   capture. The handle is trusted only when the live window still carries the
   captured pid and exe, so a recycled handle — or a snapshot persisted from
   a previous boot, where handles are meaningless — degrades to pass 2
   instead of claiming an unrelated window.
2. **Scored identity, one-to-one.** Remaining candidate pairs are scored with
   `workspaces::score_rule`, sorted, and consumed from both sides. A plain
   best-match-per-window pass would send every Brave window to the same entry
   and stack them. Placement reuses `workspaces::apply_rule_to_window` with an explicit
`target_hmon` — resolved from the stable id, not guessed from possibly-stale
coordinates — so the snap-half correction and DWM shadow-margin compensation in
[`dwm.md`](dwm.md) §3 apply unchanged.

The maximized branch honours `target_hmon` too, via `normal_pos_on_monitor`.
`SetWindowPlacement(SW_SHOWMAXIMIZED)` picks the display from
`rcNormalPosition`, and Chromium/Electron apps keep a degenerate restore-down
rect anchored at (0,0) — 647x154 and 750x155 were observed in the wild. Without
the correction, any maximized Chromium window belonging to a secondary monitor
would maximize onto the primary. The rect is left untouched when its centre is
already on the intended monitor, so the un-maximize position stays exact.

**A maximized window is glued to the monitor it is maximized on.** That
`SetWindowPlacement(SW_SHOWMAXIMIZED)` rule above holds only for a window that
is not currently maximized: on an *already-maximized* window the call updates
nothing but the restore-down rect — the OS re-evaluates which monitor to
maximize onto solely during a restore→maximize transition. Every
cross-monitor push of a maximized window is therefore a silent no-op unless
the transition is forced. This hid behind both enforcement mechanisms for a
full debugging session: a maximized Brave window kept "being pushed" to its
restored monitor (the log dutifully said so) while physically never leaving
the other one, so the scan re-detected the drift every second until the
enforcement window expired and the drift got adopted. `apply_rule_to_window`
now detects the case (`IsZoomed` + `MonitorFromWindow` disagreeing with the
target) and forces the transition: `SetWindowPlacement(SW_SHOWNOACTIVATE)`
onto the target monitor's normal rect, then
`SetWindowPlacement(SW_SHOWMAXIMIZED)` — both non-activating, wrapped in
`AnimationGuard` so the intermediate restore does not animate. Windows
already maximized on the right monitor keep the single-call path.

Why maximized windows drift in the first place: while a monitor is detached,
the survivor is primary at (0,0); when the detached monitor returns and
reclaims that origin, any window maximized over it stays glued to whatever
monitor now covers its position. No sweep or app misbehavior required — the
anchor itself moves.

Finally each monitor's `current` space is restored and visibility reapplied.

Capture reuses `workspaces::capture_active_workspace`, so window fingerprinting,
naming and snap detection have exactly one implementation.

## 5. What is deliberately not done

- **No attempt to pre-empt the teardown.** There is no reliable "monitors are
  about to go" signal; continuous shadowing is what makes the pre-teardown
  layout available.
- **`display_index` is still an enumeration ordinal** in `settings.json`. The
  startup and tray/IPC "Restore Workspace" placement now pass the monitor that
  ordinal names as `target_hmon` instead of inferring it from the rect centre,
  so placement no longer follows stale coordinates onto the wrong display — but
  the ordinal itself still drifts when the monitor set changes. Only the
  snapshot path uses stable ids end to end.
- **`MAX_MONITORS` is still 8**, but a 9th display now logs a warning instead of
  being dropped silently.

## 6. Non-code mitigations

These remove the churn rather than repairing it, and are the user's call:

- **Cutting monitor power drops hot-plug-detect on DisplayPort, but usually not
  on HDMI.** DP deasserts HPD when the panel powers down and Windows treats that
  as an unplug; HDMI's HPD is asserted by the sink off the source's +5V, which
  most monitors hold in standby. Verified on the development machine: with the
  smart plug off, the HDMI-connected HP ZR2440w still reports
  `WmiMonitorConnectionParams.Active = True`, while the DP-connected BenQ
  RD280U disappears from `Get-PnpDevice -Class Monitor`.

  The practical consequence is that a mixed-link desk produces a **stable**
  reduced topology (here: HP-only) rather than a transient or empty one, so it
  gets its own signature and its own snapshot slot like any other arrangement.
  Two ways to avoid the change altogether, if that is ever wanted: move the
  DP-connected monitor to HDMI (bandwidth permitting — 3840x2560 is near
  HDMI 2.0's limit), or power the panels down over DDC/CI (VCP `D6`: `1` = on,
  `4` = off) instead of cutting mains power. Not every panel implements `D6`.
- **`mstsc` swaps the topology regardless of monitor power.** Only a same-session
  mirroring tool (RustDesk/AnyDesk in console mode) leaves the topology alone.
  The daemon is built to work correctly either way.

## See also

- [`dwm.md`](dwm.md) §5 for the hiding state that survives a topology change.
- [`ipc-and-config.md`](ipc-and-config.md) §5 for the `layouts.json` schema.
- [`tiling.md`](tiling.md) §9 for what tiling state survives a change.
- [`user-guide.md`](user-guide.md) §7.6 for what the user sees.
