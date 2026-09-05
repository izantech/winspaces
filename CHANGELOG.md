# Changelog

All notable changes to WinSpaces are recorded here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/) and the project uses
[Semantic Versioning](https://semver.org/). `dev release <version>` moves the
Unreleased section under a new heading, bumps `Cargo.toml` and creates the tag.

## [Unreleased]

### Added

- Per-monitor independent spaces (1-9 per display, dynamic), switched by
  hotkey, tray menu or Mission Control, with a transient "Space N" indicator.
- Mission Control: live DWM thumbnails, drag-and-drop window relocation,
  space reordering, add/remove space, sticky (pinned) windows, keyboard
  navigation; replaces `Win+Tab`.
- Automatic space switching when a window is activated from the taskbar or
  by another application.
- Hyprland-style dwindle tiling: global toggle, directional focus and swap,
  split ratio, split orientation toggle with ghost preview, drag-swap and
  border drag-resize, float toggle and float rules, maximize as fullscreen,
  self-healing floats for windows that resist their tile.
- Workspace rules: capture the current layout and restore it on startup;
  per-topology layout memory that survives monitor hotplug, dock/undock and
  RDP sessions.
- Native Windows 11-style settings window (Mica, light/dark, hotkey recorder,
  autostart toggle) inside the same binary (`winspaces.exe --settings`).
- Acrylic tray context menu with per-monitor space submenus; classic menu on
  Windows 10.
- Opt-in elevated mode (scheduled task) so administrator windows can be
  managed.
- English and Spanish UI following the Windows display language.
- Inno Setup installer with graceful in-place upgrade, `--exit`/`--kill`,
  `--mission-control`, `--tiling-toggle`, `--dump` and `--restart` CLI flags,
  and `scripts\recover-windows.ps1` for crash recovery.

### Changed

- `dev check` now also lints tests (`clippy --all-targets`), builds every
  crate on its own and audits `windows-sys` feature declarations
  (`dev features`); CI runs the same gate on every push and pull request.

### Fixed

- `dev dist` and the tag-triggered release workflow read the version from
  `cargo metadata`; they had been broken since the workspace split moved the
  number to `[workspace.package]`.
- `winspaces-core`, `winspaces-ui` and `winspaces-win32` declare every
  `windows-sys` feature they use instead of relying on sibling crates.

[Unreleased]: https://github.com/izantech/winspaces/commits/main
