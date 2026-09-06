# WinSpaces documentation

Architecture and reference pages for people (and agents) working on the
code. End users start with the [user guide](user-guide.md); contributors
start with [`AGENTS.md`](../AGENTS.md) for the working agreement and then
read the pages below in the order given.

## Reading order

1. [Crate layout](crate-layout.md) — the five crates, what belongs in each,
   the one-way dependency rule and the `windows-sys` feature audit.
2. [IPC and configuration](ipc-and-config.md) — the daemon's message window,
   the `WM_WINSPACES_*` messages, every CLI flag, `settings.json`,
   `layouts.json`, and the elevation posture.
3. [DWM and window hiding](dwm.md) — shadow margins, flush snapping maths,
   window identity, and the cloaking design that hides a space's windows
   (with its crash-recovery contract).
4. [Mission Control](mission-control.md) — the overlay, the `McHost`
   indirection, `Win+Tab` interception, and the window eligibility rules.
5. [Tiling](tiling.md) — the dwindle engine, mouse gestures, the verify
   sweep and auto-floating, maximize as fullscreen.
6. [Display topology](display-topology.md) — stable monitor identity, RDP,
   the debounced reconcile, and per-topology layout restore.
7. [Tray and menu](tray-and-menu.md), [settings window](settings-ui.md),
   [space indicator](space-indicator.md) — the owner-drawn surfaces and the
   GDI + DWM recipe they share.
8. [Internationalisation](i18n.md) — locale tables, the `t`/`tr!` API, and
   how to add a language.
9. [Benchmarks](benchmarks.md) — `dev bench`, the versioned suite that
   measures the daemon's cost, and the latest numbers. The only page that
   carries performance figures.
10. [Distribution](distribution.md) — installer, signing, `dev release`, CI.

## Rejected alternatives

Design decisions are recorded next to the thing they concern, not in a
central log. The sections that answer "why not X":

- [`dwm.md` §5.4](dwm.md) — why cloaking, and not `SW_HIDE`, minimize,
  native virtual desktops or `SetCloak` alone.
- [`display-topology.md` §5](display-topology.md) — what is deliberately not
  done about RDP and monitor churn.
- [`mission-control.md` §1.1](mission-control.md) — fn pointers, not posted
  messages, for the host indirection.
- [`settings-ui.md` §5](settings-ui.md) — why no UI framework.
- [`tray-and-menu.md` §5](tray-and-menu.md) — what keeps the menu cheap.
- [`space-indicator.md` §2](space-indicator.md) — the one layered window.
- [`benchmarks.md` §6](benchmarks.md) — optimisations measured and rejected.
- [`i18n.md` §9](i18n.md) — what is deliberately not translated.
- [`distribution.md` §1 and §3](distribution.md) — Inno rather than MSIX;
  no in-app updater yet.
- [`crate-layout.md` §2](crate-layout.md) — why there is no shared
  `pt_in_rect` and no shared surface abstraction.
- [`reports/tiling-roadmap.md`](../reports/tiling-roadmap.md) — tiling
  features parked or skipped, with reasons.

## Reports

[`reports/`](../reports) holds point-in-time records: reviews, incident
investigations, research, checklists. Each starts with a `Status:` line
saying whether it is still open, partially delivered or superseded. They are
not authoritative; when a report and a page here disagree, the page wins and
the report needs a status update.

## Page conventions

- One topic per page, kebab-case file name.
- An H1, a one-paragraph summary, then `*Last verified: <date>, against
  <commit>.*` Bump the line when you re-read the page against the code, not
  only when you edit it.
- Numbered `## N.` sections so other pages can cite `§N`; subsections are
  `### N.M`.
- A `## See also` footer listing the related pages.
- Numbers (RAM, CPU, latency, binary size) live only in
  [`benchmarks.md`](benchmarks.md); everywhere else links there.
- Written in English.
