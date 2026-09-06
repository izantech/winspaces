# Commercial readiness roadmap

**Status:** active gap analysis (2026-08-30). Item 1.5 (localisation) shipped in `2af7449`; item 0.3 has a `LICENSE` since `c2911e6`; the rest is open.

Written 2026-08-30 against `feature/window-tile-manager` at `80fefde`. This is a
gap analysis, not a feature wishlist: the question it answers is "what stands
between the current build and a product someone pays for". Items are grouped
by how hard they block a sale, and ordered within each group by value.

## Where the product stands

Technically the daemon is ahead of everything that charges money on Windows
(DisplayFusion ~34 $, AquaSnap ~18 $, Stardock Groupy/Fences) and of everything
that is free (PowerToys FancyZones, komorebi, GlazeWM):

- per-monitor independent spaces (1-9, dynamic), the one thing Windows' own
  virtual desktops refuse to do;
- a native Mission Control with live DWM thumbnails, drag-to-space, reorder,
  pin;
- dwindle tiling with mouse gestures, split toggle, maximize-as-fullscreen and
  self-healing floats;
- workspace rules with layout save/restore keyed by monitor topology, RDP and
  dock/undock survival;
- < 3 MB RAM, single native binary, Inno installer with hot upgrade, opt-in
  elevated mode.

The moat is the **macOS Spaces experience on Windows** — per-monitor spaces,
Mission Control, polish — not the tiler, which free tools already cover. The
gaps below are infrastructure, trust and finish, not engine work.

## Tier 0 — blockers: nothing can be sold without these

| # | Gap | Why it blocks | Notes |
|---|---|---|---|
| 0.1 | **OV/EV code-signing certificate** | SmartScreen blocks the unsigned installer; paying users never get past the download | Purchase + identity verification; the signing plumbing (`make-installer.ps1`, `WINSPACES_SIGN_THUMBPRINT`) already works. See `docs/distribution.md` §2 |
| 0.2 | **Licensing and checkout** | There is no key, trial, activation or purchase flow at all | Merchant of record (Paddle / Lemon Squeezy) handles VAT and refunds. Offline-verifiable signed keys (Ed25519 over `email+expiry+edition`), a time-limited trial, and an unlicensed state that degrades gracefully (e.g. spaces stay, tiling and rules pause) |
| 0.3 | **Legal** | No `LICENSE`, EULA or privacy policy in the repo | Needed by every checkout provider and store. Even with zero telemetry the privacy policy must say so |
| 0.4 | **In-app auto-update** | "Check for Updates" opens GitHub Releases; paying users expect a one-click update | Deliberately not built so far (`docs/distribution.md` §3). Needs a tiny HTTP client, a `latest.json` endpoint, signature check, and the existing hot-upgrade path of the installer |
| 0.5 | **Support and diagnostics** | Every ticket is a five-mail exchange without a diagnostic bundle | "Export diagnostics" button: log, settings, layouts, monitor topology, Windows build, daemon elevation state. Crash capture (minidump on unhandled exception, `SetUnhandledExceptionFilter`) |
| 0.6 | **Onboarding** | First run is a tray icon and silence; default hotkeys are 3-4-modifier chords | Welcome window on first run, a `?` shortcut overlay, defaults that do not collide with common apps, a "what changed" note after updates |

## Tier 1 — robustness a paying customer hits in the first week

| # | Gap | Risk | Notes |
|---|---|---|---|
| 1.1 | **Undocumented shell-cloak COM surface** | A vtable change in a Windows update makes every managed window vanish on every customer machine at once | Per-build canary at startup (cloak/uncloak a probe window and verify), automatic fallback to `SW_FORCEMINIMIZE`, a compatibility matrix (23H2 / 24H2 / 25H2 / Insider) in CI on VMs. See `docs/dwm.md` §5.6 |
| 1.2 | **Watchdog and panic key** | Daemon death leaves windows cloaked until the user finds `recover-windows.ps1` | Guardian process or scheduled task that relaunches the daemon; a global "show everything" hotkey that works even from a hung daemon (separate tiny process); `reclaim_orphaned_windows` already covers the restart path |
| 1.3 | **ARM64 build** | Snapdragon X / Surface machines are a real share of new laptops | One more CI target (`aarch64-pc-windows-msvc`); installer needs an ARM64 payload branch |
| 1.4 | **Accessibility** | Settings, tray menu and Mission Control are hand-drawn: no UIA, no screen reader, keyboard nav partial | Store listings and any business sale reject this. Minimum: UIA provider for the settings window, full keyboard operation of Mission Control, focus visuals |
| 1.5 | **Localisation** | All strings hard-coded in English | String table + locale detection; Spanish and German first (largest paying utility markets after English) |
| 1.6 | **Elevated windows story** | Non-elevated daemon cannot manage admin windows; elevated mode is opt-in and one UAC | Acceptable, but the settings page must explain the trade-off in one sentence; today it is a toggle with a technical description |
| 1.7 | **Native virtual desktops coexistence** | Users arriving with existing Task View desktops get two competing systems | Detect native desktops on first run, offer migration or a clear "WinSpaces replaces Task View" choice |

## Tier 2 — features that sell to the target niche

| # | Feature | Why | Notes |
|---|---|---|---|
| 2.1 | **Touchpad gestures** | 3/4-finger swipe between spaces and pinch for Mission Control is *the* feature for macOS switchers; nobody on Windows does it well | Windows only maps gestures to predefined actions, so this needs raw Precision Touchpad input (`WM_INPUT` HID, usage page 0x0D). Forgiving-gesture rules apply (commit on the meaningful axis) |
| 2.2 | **Space switch animation** | Today the switch is a hard cut; the "feels like a product" moment comes from a slide | Capture-and-slide with DWM thumbnails of both spaces, 150-200 ms, disabled under reduced-motion |
| 2.3 | **Named spaces, per-space wallpaper** | macOS parity; makes spaces memorable | Names in Mission Control cards and the tray badge; wallpaper via `IDesktopWallpaper` per monitor on switch |
| 2.4 | **Mission Control search and keyboard** | Type-to-filter windows, arrow navigation, app grouping | Turns Mission Control into the app switcher |
| 2.5 | **Tiling: master-stack layout, per-app "never tile", persisted per-space layouts** | Table stakes against komorebi/GlazeWM users | `LayoutKind::MasterStack` is already reserved; float rules exist but need a "from this window" affordance |
| 2.6 | **Profiles and sync** | Export/import settings, optional sync between machines | JSON already; add a bundle format and a "sync folder" (OneDrive/Dropbox) rather than a server |

## Tier 3 — business

- **Pricing.** Either one-time 20-30 $ with a year of updates (what the
  Windows utility market accepts without friction), or Free (spaces + Mission
  Control) / Pro (tiling, rules, gestures, profiles) for a funnel. Decide
  before 0.2, because the unlicensed state depends on it.
- **Website and user docs.** `docs/` is architecture documentation for
  developers. Users need screenshots, a 60-second video, a shortcut guide and
  a FAQ ("my windows disappeared", "admin windows", "RDP").
- **Distribution.** winget and Chocolatey manifests (installer already
  satisfies their requirements, `docs/distribution.md` §4). Microsoft Store
  only if MSIX containment restrictions are accepted — currently rejected for
  cause.
- **Feedback loop.** Public issue tracker or support mail, opt-in anonymous
  telemetry limited to version, Windows build, monitor count and crash
  counts — enough to prioritise 1.1 without reading users' logs.

## Suggested order

1. 0.1 certificate and 0.3 legal (purchases and paperwork; start now, they
   have lead time).
2. 0.5 diagnostics + 1.2 watchdog (protects the reputation the day the first
   customer's windows vanish).
3. 0.2 licensing + 0.4 updates (the shop).
4. 0.6 onboarding + 1.4 accessibility minimum + 1.5 Spanish.
5. 2.1 gestures + 2.2 animation (the demo video).
6. Everything else by demand.
