# WinSpaces Roadmap

Prioritized from the August 2026 full-codebase review and subsequent cloaking research. Ordered by value against effort — items are expected to be tackled roughly top to bottom.

## 1. Revamp `is_valid_window` filtering (correctness, small)

`is_valid_window` (`crates/winspaces-daemon/src/desktop.rs`) still excludes windows whose *title contains* substrings like "input experience" or "task host window", plus hardcoded localized strings ("experiencia de entrada"). A legitimate user window whose title happens to contain one of these (e.g. a browser window titled "Windows Input Experience — Search Results") silently becomes unmanageable: skipped by scans, invisible to Mission Control, stranded during space switches.

Goal: replace the title/class blacklists with a principled eligibility check modeled on the shell's own Alt-Tab rules (ownership chain, extended styles, cloak state) so that system windows are excluded by *what they are*, not what their title says. Titles should at most remain as a last-resort exact-match list. Research first — see `.local/handoff/is-valid-window-revamp.md`.

## 2. Mission Control drag visual feedback (UX, medium)

Dragging a thumbnail onto a Space card currently gives no visual indication until the drop lands. Add a ghost/outline that follows the cursor and a hover highlight on the target Space card. Contained in `crates/winspaces-daemon/src/mission_control.rs`, which already owns its own paint loop.

## 3. Decide the elevation posture (product decision, small code)

The daemon currently runs elevated via `dev run`; that is why the UIPI message filter exists and is the only way windows of elevated apps get managed (see `docs/dwm.md` §5.5). Shipping "always run as admin" is a hard sell. Options:

- Run non-elevated; document that elevated apps' windows are unmanaged.
- Keep elevation but launch through a scheduled task so there is no UAC prompt at login.

Settle this before packaging (item 5), since the installer has to encode the choice.

## 4. GUI XAML/theming migration (large, deferred)

The configurator is imperative code-behind with hardcoded dark ARGB brushes — no XAML, no theme resources, no light theme. It works; migrate only when a light theme or an accessibility pass actually matters. Do not start this before items 1–3.

## 5. Distribution: installer, signing, updates (product, large)

The biggest gap for a commercial app: no installer, no code signing, no update story. Needs an installer (MSIX or Inno/WiX), a signing certificate, autostart wiring through the installer rather than the GUI toggle alone, and at minimum a manual "check for updates" affordance. Depends on item 3 for the elevation choice.

---

Settled decisions that should not be revisited: tray left-click is an instant Mission Control toggle (no double-click action, no delay timer); window hiding stays on documented `DWMWA_CLOAK` + `SW_FORCEMINIMIZE` (alternatives evaluated and rejected in `docs/dwm.md` §5.4).
