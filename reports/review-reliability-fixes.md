# Review — cross-monitor / new-window reliability fixes (uncommitted, `feature/window-tile-manager`)

Scope: working-tree diff over `28e7a78` — `engine.rs`, `hooks.rs`, `app.rs`, `shell.rs`, `main.rs`, `docs/tiling.md`.
Verified locally: `cargo fmt --check` clean, `cargo clippy --workspace --all-targets` clean,
`cargo test --workspace` green (87 core / 40 ui / 13 bin). Green build, several real defects.

## Verdict

The diagnosis is broadly right: a tiled window dragged cross-monitor was classified against the
*origin* monitor's tiles and snapped back, and new windows really were only picked up by
`scan_untracked_windows()` on a later space switch. The direction of the fix is correct.

The implementation was not landable as reviewed. One defect could permanently destroy other apps'
UI; two others reproduced the exact symptoms the change was meant to remove.

**Status: B1, B2 and B3 are fixed in this working tree** (see the per-item *Fixed* notes below).
`.\dev check` is clean: fmt, `clippy -D warnings`, and 88 core / 40 ui / 24 common / 13 bin tests.
S1–S4 and the Minor items are still open.

---

## Blocking — fixed

### B1 — `EVENT_OBJECT_SHOW` hook will track and cloak **child controls**

`shell.rs:436` filters `id_object != 0 || id_child != 0`, i.e. `OBJID_WINDOW`/`CHILDID_SELF`.
That filter does **not** restrict to top-level windows: `ShowWindow` on any child control emits
`EVENT_OBJECT_SHOW` with exactly those ids for the child HWND.

`is_valid_window` has never needed a `WS_CHILD` check — `WS_CHILD` appears nowhere in the tree —
because every existing caller is fed top-level handles only (`EnumWindows`, the ShellHook,
the foreground/minimize/movesize WinEvents). Walk a child control through
`eligibility.rs:69` `passes_structural_checks`:

- no `WS_EX_TOOLWINDOW`, no `WS_EX_NOACTIVATE` — passes
- class not in `is_shell_class` (`Button`, `Edit`, `SysTabControl32`, … are not listed) — passes
- `has_title` — a `Button`/`Static`/`Edit` with text returns >0 from `GetWindowTextW` — passes
- `WS_VISIBLE` — it was just shown — passes
- owner: `GetAncestor(child, GA_ROOTOWNER)` returns the top-level parent, which itself passes

So `is_valid_window(child) == true`, and `show_hook_proc` calls `track_window` on it.

Consequences, in order of severity: the child is added to a space list and gets the WinSpaces
state prop; `mark_tiling_dirty` fires and the tiler `SetWindowPos`es a control inside its parent;
and on the next space switch `set_window_visibility(false)` DWM-cloaks + `SW_HIDE`s the control.
The owning app will never re-show it. That is a permanent, user-visible corruption of a third-party
window, and per the hidden-state invariant the prop then has to be carried correctly to ever undo it.

**Fixed.** Both layers, since they cover different things:

- `eligibility.rs:69` — `passes_structural_checks` now rejects `WS_CHILD` as its first test. This
  hardens every caller, not just the new hook, and closes the implicit
  "callers only ever pass top-level handles" invariant that was never written down.
- `shell.rs:436` — `show_hook_proc` bails when `GetAncestor(hwnd, GA_ROOT) != hwnd`, before it
  touches the state lock. `GA_ROOT` walks the parent chain, not the owner chain, so owned
  top-level dialogs still pass. This is also the cheapest available cull on a very
  high-frequency event, which takes a bite out of S4.

Covered by `child_control_is_excluded`, which builds the facts exactly as the event delivers
them (visible, titled, `Button`, top-level parent as owner) and asserts both `is_eligible` and
`is_tile_eligible` reject it. It fails without the `WS_CHILD` check.

### B2 — cross-monitor drags are gated on `is_settling()`, which every retile sets

`shell.rs:547` guards the cross-monitor branch with
`!reconcile_pending && !is_settling() && !try_enforce_restore(hwnd)`, copied from the scan.
In the scan those guards are right: the scan infers intent from geometry, so it must not treat
not-yet-applied bulk placement as a drag. `EVENT_SYSTEM_MOVESIZEEND` is the opposite — it is a
direct user gesture and the strongest possible evidence of intent.

`flush_retile` ends with `begin_settle(500)` (`engine.rs:251`), and `flush_retile` runs on the
50 ms coalesced timer after *any* dirty space. So any retile within 500 ms of the drop silently
discards the move. Worse, `tiling_drag` has already been set to `None` on the line above, so
`tiling_on_movesize_end` never runs either: no re-home, no snap-back, no retile. The window sits
on monitor 2 while still tracked to monitor 1 until an unrelated scan fires — which is
*precisely* "I can't move windows to my secondary monitor, sometimes I have to switch spaces".

It also self-compounds: a failed attempt snap-backs → dirty → retile → another 500 ms of settling,
so retrying immediately fails again.

**Fixed.** `is_settling()` is gone from the movesize path; `reconcile_pending` and
`try_enforce_restore` stay, since those two genuinely mean "this geometry is not the user's".
The branch now yields a `rehomed` bool, and when the re-home is refused the gesture falls through
to `tiling_on_movesize_end` instead of being swallowed — so the window snaps back to its tile
rather than sitting on a display we declined to adopt. That also restores the pre-change invariant
that every MOVESIZEEND resolves `tiling_drag`, which the swallowing branches had broken.

The asymmetry with `scan_untracked_windows` (which does check `is_settling()`) is now spelled out
in a comment at the call site, so the next reader doesn't "restore" it.

### B3 — the cursor-monitor check breaks edge resizing on boundary-adjacent tiles

`engine.rs:608-630` re-homes the window whenever the *cursor* at drop time is on another monitor.
`EVENT_SYSTEM_MOVESIZEEND` fires for resizes, not just moves. Dragging the right edge of the
rightmost tile on monitor 1 rightwards necessarily puts the cursor on monitor 2 while the window
stays on monitor 1 — so `AdjustDwindleRatio` is never reached and the window teleports to
monitor 2's current space instead. Ratio adjustment is dead on every monitor-adjacent edge.

This also contradicts the forgiving-drag-gesture rule: the gesture is classified on the axis the
cursor strayed along rather than on what the drag actually meant.

**Fixed** by deleting *both* new cross-monitor blocks from `tiling_on_movesize_end`, rather than by
qualifying the cursor check. `monitor_index_for_hwnd` uses `MONITOR_DEFAULTTONEAREST`, i.e. the
largest-overlap monitor, so the daemon-level check in `movesize_hook_proc` already draws the line
in the right place: a window the user actually moved ends up mostly on the target display and is
re-homed, while a window whose cursor merely strayed across the boundary stays put and reaches
`classify_drag`. Edge resizing on monitor-adjacent tiles works again, and this is the forgiving
behaviour the drag-gesture rule asks for.

Removing them also fixes the guardless copy flagged in S1 and makes the B2 fall-through safe —
otherwise handing a refused gesture to the engine would have re-homed the window through the very
path whose guards we had just decided applied.

---

## Should fix before landing

### S1 — the cross-monitor re-home already exists; this adds three more copies

`manager.rs:920-950` inside `scan_untracked_windows` is the canonical implementation, down to the
log string `"Window {:?} moved across displays from Mon {} to Mon {} (Space {})"`, which is now
duplicated verbatim in `shell.rs`. The `engine.rs` copies are gone with B3, leaving three:

| site | `is_valid_window` | `reconcile_pending` | `is_settling` | `try_enforce_restore` |
|---|---|---|---|---|
| `manager.rs` scan | yes | yes | yes | yes |
| `shell.rs` activation | no | yes | yes | yes |
| `shell.rs` movesize | no | yes | no (correct, see B2) | yes |

The remaining divergence is defensible — the scan infers intent, the two event handlers observe it
— but it is currently three hand-maintained copies that happen to agree, not one decision with two
callers. The activation path's `is_settling()` is worth a second look on the same reasoning as B2,
though it is far less likely to bite there.

The same applies to the workspace-rule block: `shell.rs:231-260`, `shell.rs:450-480` and the
existing `HSHELL_WINDOWCREATED` arm at `shell.rs:106-135` are three near-identical ~35-line copies.

Extract one `SpaceManager::rehome_if_moved(&mut self, hwnd) -> bool` and one
`try_place_by_rule(state, hwnd) -> bool`, and call those from every site.

### S2 — `Win+Shift+←/→` is not actually caught

The claim that `shell.rs:293` catches the Windows move-to-monitor shortcut doesn't hold up:
that shortcut runs no modal move loop (no `MOVESIZESTART`/`END`) and does not change the
foreground window, so neither new path fires. It will be picked up on the *next* activation of
that window, or by the next scan — i.e. the same latency as before. Either drop the claim or
handle it properly (`EVENT_OBJECT_LOCATIONCHANGE` on the foreground window, debounced, or a
targeted re-home on the existing retile timer).

### S3 — verify-sweep tolerance: the stated reason is wrong and the doc is half-updated

`engine.rs:282` raises the tolerance 2px → 8px "for DPI scaling". Both `DWMWA_EXTENDED_FRAME_BOUNDS`
and the expected rect derived from the monitor work area are physical pixels; a genuine DPI
mismatch produces errors far larger than 8px, so this does not address that cause. The real
sources of small deltas are min-size constraints and rounding. Fine as a heuristic, but fix the
comment — and note that with small `gaps.inner` an 8px slack is visible misalignment the sweep now
accepts.

The strike change 2 → 4 is reasonable (auto-float now takes ~800 ms instead of ~400 ms) and the
reset-on-success path at `engine.rs:314` is intact.

`docs/tiling.md` was updated, but the doc comment on `verify_retile` itself (`engine.rs:254-256`)
still says "resist twice" and "after 3 attempts".

### S4 — the show hook is installed unconditionally and is a high-frequency firehose

Every `EVENT_OBJECT_SHOW` system-wide reaches `show_hook_proc`. The `find_window` scan first is
the right ordering, but on a miss it runs `is_valid_window` → `gather_window_facts`, which is
`GetClassNameW` + `GetWindowTextW` + a cross-process `DwmGetWindowAttribute(DWMWA_CLOAKED)`, plus a
possible `match_rule_for_window` (process-image lookup). Menus, flyouts and tooltips fire this
constantly. Gate the hook on `tiling_enabled || auto_restore_workspaces`, or at minimum bail early
on non-top-level handles (which B1 requires anyway) — that removes most of the traffic.

`WINEVENT_OUTOFCONTEXT | WINEVENT_SKIPOWNPROCESS` is correct here, and `with_app_state`'s
`try_borrow_mut` means re-entrancy degrades to a dropped event rather than a panic. No issue there.

---

## Minor

- **`app.rs:181-201` — delete the new test.** `settings_command_spawns_with_null_stdio` calls
  `current_exe()` inside a unit test, so it spawns *the test harness binary* with `--dump NUL`
  (libtest rejects the flag and exits). It exercises none of `launch_settings`, asserts only that
  `spawn` succeeded, and spawns a process on every `cargo test`.
- **`engine.rs:1080` — `tiling_on_movesize_end_with_no_drag_is_noop` asserts nothing new.**
  `tiling_drag` was already `None` on entry. None of B1–B3 or the cross-monitor logic has coverage;
  the "87 tests passing" line in the summary is not evidence these fixes work.
- **`main.rs:165-176` + `app.rs:140-170` — scope creep.** The `SetStdHandle(NULL)` /
  `Stdio::null()` / `ShellExecuteW` fallback trio is follow-up to `28e7a78`, unrelated to the
  reported window-manager symptoms. The `SetStdHandle` calls are the reason `Stdio::null()` became
  necessary; `launch_settings` is the only `Command::new` in the tree, so it's self-consistent, but
  it should be a separate commit and any future spawn must remember the explicit stdio.
- **`shell.rs:509` — `minimize_hook_proc` restructure is fine** (`track_window` marks the space
  dirty itself), and `MINIMIZESTART`/`END` are top-level only, so B1 does not apply there.
- **`app.rs:50` — `_show_hook` field placement.** The struct's doc comment flags field order as
  drop order; inserting between `_win_event_hook` and `_minimize_hook` is harmless, but it is a
  documented-hazard field and worth a deliberate look.

## Remaining work

1. ~~B1, B2, B3~~ — done.
2. S1 — extract `rehome_if_moved` + `try_place_by_rule`; collapse the three copies of each.
3. S2 — drop the `Win+Shift+←/→` claim, or handle it for real.
4. S3/S4 — tolerance comment, the stale `verify_retile` doc comment, hook gating.
5. Replace both of Gemini's new tests with coverage of the re-home helper.
6. Re-run the manual shakedown in `reports/tiling-shakedown.md`. B1–B3 are all
   observation-only bugs — nothing here is provable from unit tests, so this is the real gate.
   Specifically worth driving: dragging a tiled window across the boundary repeatedly and fast
   (B2 was timing-dependent), resizing the outer edge of a monitor-adjacent tile (B3), and
   opening apps with heavy child-window churn — Explorer, a settings dialog, anything with tab
   pages — while watching for controls that vanish after a space switch (B1). Include a
   2→1→2 display cycle.
