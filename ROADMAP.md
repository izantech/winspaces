# WinSpaces Roadmap

Prioritized from the August 2026 full-codebase review and subsequent cloaking research. Ordered by value against effort — items are expected to be tackled roughly top to bottom.

## 1. Mission Control drag visual feedback (UX, medium)

Dragging a thumbnail onto a Space card currently gives no visual indication until the drop lands. Add a ghost/outline that follows the cursor and a hover highlight on the target Space card. Contained in `crates/winspaces-daemon/src/mission_control.rs`, which already owns its own paint loop.

## 2. Decide the elevation posture (product decision, small code)

The daemon currently runs elevated via `dev run`; that is why the UIPI message filter exists and is the only way windows of elevated apps get managed (see `docs/dwm.md` §5.5). Shipping "always run as admin" is a hard sell. Options:

- Run non-elevated; document that elevated apps' windows are unmanaged.
- Keep elevation but launch through a scheduled task so there is no UAC prompt at login.

Settle this before packaging (item 4), since the installer has to encode the choice.

## 3. GUI XAML/theming migration (large, deferred)

The configurator is imperative code-behind with hardcoded dark ARGB brushes — no XAML, no theme resources, no light theme. It works; migrate only when a light theme or an accessibility pass actually matters. Do not start this before items 1–2.

## 4. Distribution: installer, signing, updates (product, large)

The biggest gap for a commercial app: no installer, no code signing, no update story. Needs an installer (MSIX or Inno/WiX), a signing certificate, autostart wiring through the installer rather than the GUI toggle alone, and at minimum a manual "check for updates" affordance. Depends on item 2 for the elevation choice.

---

Settled decisions that should not be revisited: tray left-click is an instant Mission Control toggle (no double-click action, no delay timer); window hiding stays on documented `DWMWA_CLOAK` + `SW_FORCEMINIMIZE` (alternatives evaluated and rejected in `docs/dwm.md` §5.4); window eligibility is structural — Alt-Tab-style owner-chain/style/class/cloak rules with a pure tested `is_eligible` (`docs/mission-control.md` §5) — never substring or localized title matching.
