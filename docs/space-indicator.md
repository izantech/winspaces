# The Space Indicator

The transient **"Space N"** panel that flashes near the taskbar of the display
whose space just changed — WinSpaces' answer to the toast Windows shows when
you switch its own virtual desktops, adapted to the per-monitor model.

This page covers the one thing about it that will look wrong to anyone reading
the code next to its sibling surfaces: it is a **layered** window, while the
tray flyout, Mission Control and the settings window are all DWM-backdropped,
and [`tray-and-menu.md`](tray-and-menu.md) §2 says `WS_EX_LAYERED` must never
be added. That rule still holds — for backdrop windows. This is the deliberate
exception, and §2 below is why.

---

## 1. What Triggers It

Every space switch, from every trigger: the `Alt+1..9` hotkeys, prev/next,
the tray menu's "Display N > Space M", Mission Control's space cards and digit
keys, and the automatic switches that follow a taskbar click or an app
activation.

That coverage comes from hooking the one function that actually changes a
space rather than the callers that reach it. `DesktopManager::switch_desktop`
(`winspaces-core`) is the sole mutator; every trigger funnels through it. But
`winspaces-core` sits *below* `winspaces-ui` and must never call into it, so
the direction is inverted the same way Mission Control's `McHost` vtable
inverts its own dependency (see [`mission-control.md`](mission-control.md)
§1.1): `winspaces-core` exposes `set_switch_observer(fn(&SwitchNotice))`, and
the bin — the only crate that can name both sides — installs
`winspaces_ui::space_indicator::on_space_switch` at startup, next to
`install_host`.

Two properties fall out of hooking the choke point instead of the call sites:

1. **A future trigger cannot forget to notify.** Anything that switches a
   space gets the indicator for free.
2. **The switches that must *not* toast are excluded by construction.** The
   observer only fires when `old_desk != target_desk`, and the two callers
   that re-issue a switch for reasons other than a user switching — the
   workspace restore pass, which re-applies visibility to the space already
   current, and `remove_space`, whose `current` is already remapped by the
   time it calls through — both pass the same index and are silently skipped.
   No suppression flag, no caller opt-out list.

`SwitchNotice` carries the monitor's work rect by value rather than letting
the observer look it up, because the observer runs *inside* the caller's
`AppState` borrow: reaching back into daemon state would hit `with_app_state`'s
re-entrancy guard and be dropped with a warning.

Suppressed while Mission Control is open — the overlay already marks the
active space with a highlighted card, and a toast painted over a full-screen
Exposé is redundant.

Config: `space_indicator` in `settings.json` (default on), surfaced as **Show
Space Indicator** on the settings window's System page.

---

## 2. Why It Is Layered

Because it is the only WinSpaces surface that has to **fade**, and a DWM
system backdrop cannot fade.

The instinct is to reuse the menu recipe — `extend_frame_full` then
`set_backdrop(Acrylic)` — and animate the painted alpha down to zero. That
does not dissolve the panel. Under a system backdrop, the painted pixels are
the only thing *hiding* the blur DWM is compositing behind the window; drop
their alpha and you don't fade out, you fade *into* a floating blurred slab
with rounded corners. There is no attribute that fades the backdrop itself.

So this surface forgoes the backdrop entirely:

| Aspect | Mechanism |
| :--- | :--- |
| Window | `WS_POPUP` + `WS_EX_LAYERED \| WS_EX_TRANSPARENT \| WS_EX_TOOLWINDOW \| WS_EX_TOPMOST \| WS_EX_NOACTIVATE` |
| Pixels | Premultiplied 32bpp top-down DIB (`DibSection`), pushed with `UpdateLayeredWindow` + `AC_SRC_OVER \| AC_SRC_ALPHA` |
| Shape | Anti-aliased rounded-rect coverage computed per pixel in `geometry::round_rect_coverage` |
| Fade | `BLENDFUNCTION::SourceConstantAlpha` per frame |
| Backdrop | **None.** No `extend_frame_full`, no `set_backdrop`, no `DWMWA_WINDOW_CORNER_PREFERENCE` |

`WS_EX_LAYERED` and the DWM backdrop therefore never meet on one HWND, which
is what the prohibition in [`tray-and-menu.md`](tray-and-menu.md) §2 actually
protects against. Nothing is lost visually: Windows' own desktop indicator is
an opaque panel too, not an acrylic one.

**`WS_EX_TRANSPARENT` is load-bearing, not cosmetic.** The panel floats
directly above the taskbar, exactly where people click. Without hit-test
pass-through, every toast would swallow a click aimed at whatever it covers,
for a second and a quarter, several times a minute.

### Consequences, all intentional

- **No DWM shadow, no `DWMWA_WINDOW_CORNER_PREFERENCE`.** Both are computed
  against the rectangular window frame, not the rounded panel painted inside
  it, so enabling either would draw a rectangle around a rounded thing.
- **The corners are cut in the DIB.** `round_rect_coverage` is a signed-
  distance function that returns fractional coverage across a one-pixel band,
  so the arcs land soft. The tray badge's equivalent mask
  (`winspaces-ui/src/tray/badge.rs`) is a hard in/out test — fine on a 16 px
  icon, visibly stepped on a 60 px panel.
- **A one-pixel border is painted into the ring** between the outer shape and
  the same shape inset by a device pixel, so the panel stays legible against a
  wallpaper that happens to match its fill.

### Why not `gdi::surface::paint_surface`

The shared alpha-managed painter promotes every GDI-touched (alpha-0) pixel to
a hardcoded opaque `0xFF`. That byte is precisely what this surface has to
vary, and its callers — the menu and the settings window — depend on the
opaque promotion. The indicator does its own final pass instead, and
`paint_surface` stays unchanged.

---

## 3. The Fade

110 ms in, 900 ms hold, 260 ms out, smoothstepped, driven by a `SetTimer` at
the target display's `frame_interval_ms` — the same per-display frame budget
Mission Control's drag throttle uses.

**A frame costs one `UpdateLayeredWindow` and no GDI at all.** The panel is
painted once per toast into a DIB that stays alive for its duration; the fade
varies only `SourceConstantAlpha`, and Windows multiplies that with the
per-pixel alpha already baked into the bitmap. So the per-pixel shape work
happens once, not sixty times a second.

A switch arriving while the panel is still up **snaps it back to full opacity
and restarts the hold** instead of replaying the fade-in. Holding `Alt+←` then
reads as one steady panel whose number changes, rather than a strobe or a
stack of overlapping toasts. There is only ever one indicator window; a switch
on a second monitor moves it.

---

## 4. Measured Cost

No window, no DC, no bitmap and no timer exist until the first space switch.
At the end of every toast the surface kills its timer, hides the window and
releases the DC and DIB, keeping only the HWND — which is reused rather than
recreated, unlike the menu's create-per-open pattern, because switches are
frequent enough that window churn would be the expensive part.

Nothing ticks between switches, so the daemon's fully event-driven idle
contract (see [`tray-and-menu.md`](tray-and-menu.md) §5) is unchanged.

Measured on a live release daemon (Ryzen 9 9900X @ 4.4 GHz, two monitors) by
running one 4-phase protocol **twice** — once with `space_indicator` disabled,
once enabled, driving identical switches by posting the tray menu's
`WM_COMMAND` — so the underlying `switch_desktop` work cancels out and the
delta is the indicator alone:

| Phase | Indicator off | Indicator on | Delta |
| :--- | ---: | ---: | ---: |
| Idle, 30 s | 1.94 Mcycles/s | 1.14 Mcycles/s | none (within noise) |
| One isolated toast | 16.3 Mcycles/switch | 50.8 Mcycles/switch | **~35 Mcycles** |
| Sustained, 2.5 switches/s | 10.5 Mcycles/switch | 23.3 Mcycles/switch | **~13 Mcycles** |

~35 Mcycles is **8 ms of CPU spread across the toast's 1.27 s life — 0.6% of
one core while visible, and nothing at all between toasts.** A 30-second
hammer at 2.5 switches/second added 0.7% of one core.

The sustained figure being *lower* per switch than the isolated one is the
re-trigger design paying off: overlapping toasts never run their full fade
cycle, so frames are amortized rather than repeated.

Handles and memory over 79 back-to-back switches:

- **GDI: 14 at rest, 16 while a toast is live, back to 14 within one second of
  the last one** — the +2 is the DIB and its memory DC, and it is returned
  every time. No staircase across the burst.
- **USER: +1 while a toast is live** (the fade timer). The floor itself drifts
  by ±1 between runs for reasons unrelated to this surface, so treat a
  single-object difference here as noise, not signal.
- **Private memory: ~0.25 MB transient peak, returning to the pre-test floor.**

The number that matters for a surface firing several times a minute is that
GDI comes back to exactly its floor, not that the peak is small — and it does.
