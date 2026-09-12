# Distribution: Installer, Signing & Updates

How WinSpaces is packaged, signed, and updated. The pipeline entry point is `.\dev dist`, which delegates to `scripts/make-installer.ps1`.

*Last verified: 2026-09-12, against 0600ea2.*

---

## 1. Installer (Inno Setup 6)

`installer/winspaces.iss`, compiled by `scripts/make-installer.ps1` into `dist/WinSpaces-Setup-x64-<version>.exe`. The version comes from `cargo metadata`, i.e. `[workspace.package].version` in the root `Cargo.toml` (every crate inherits it through `version.workspace = true`); the release tag must carry the same number — see §3.

The application icon (`crates/winspaces/winspaces.ico`, inside the crate so `cargo package` ships it; also the installer's `SetupIconFile`) and a VERSIONINFO block (product name, file version from `Cargo.toml`, copyright — what Explorer's Details tab and SmartScreen read) are compiled into the daemon exe by `crates/winspaces/build.rs`, which writes the `.rc` into `OUT_DIR` and hands it to `embed-resource` (needs `rc.exe` from the Windows SDK — already required for linking). Every window class registered through `winspaces_win32::window_class` carries it, so the settings window shows it on the taskbar; the tray icon is drawn at runtime and unaffected. `scripts/gen-icon.py` regenerates the `.ico`, `assets/logo.svg`, `site/favicon.svg` and `site/og-image.png` from one drawing — edit the spec there, never the outputs.

Design decisions:

- **Per-user install, no UAC** (`PrivilegesRequired=lowest`, installs to `%LOCALAPPDATA%\Programs\WinSpaces`). Matches the non-elevated daemon posture ([`ipc-and-config.md`](ipc-and-config.md) §6). The opt-in elevated scheduled task is *not* an installer feature — power users run `{app}\scripts\install-elevated-autostart.ps1` from an elevated shell.
- **Why Inno, not MSIX**: the daemon manipulates other processes' windows (`SetProp`, DWM cloaks, LL keyboard hook) and autostarts a tray process — all friction points under MSIX containment, and MSIX sideloading requires a trusted signature just to install. Classic setup matches the product; comparable tools (komorebi et al.) ship the same way.
- **Tiny payload, zero runtimes**: the staged payload is `winspaces.exe` (daemon + settings window in one native binary) plus two recovery/autostart scripts — 3 files, ~2 MB installer. Target machines need no runtime of any kind; the settings window ships inside the daemon exe and opens via `winspaces.exe --settings`.
- **Autostart task** (checked by default) writes the same HKCU `Run` value the settings window's autostart toggle manages, so both stay in sync with the installer's choice.
- **Upgrades**: `PrepareToInstall` posts `--exit` to a running daemon and waits for the exe file lock to release — the daemon uncloaks every managed window before files are replaced, so an upgrade can never strand hidden windows.
- **Uninstall**: runs `--exit` (same guarantee), removes the `Run` value, and deletes the elevated scheduled task if one exists. It leaves `%LOCALAPPDATA%\WinSpaces` (settings, layouts, log) in place on purpose, so a reinstall finds the user's configuration.
- **Licence page**: `LicenseFile=..\LICENSE` shows the GPLv3 license terms in the wizard; silent installs accept them implicitly.

End-to-end verification procedure (re-run after payload changes): silent install (`/VERYSILENT`), daemon + settings window launch from the install dir on a machine path with no dev runtimes involved, silent uninstall leaves nothing under `{app}`, no `Run` value, no processes. Note the uninstaller's `--exit` stops *any* running daemon, so run this when a daemon restart is acceptable.

## 2. Code Signing

`make-installer.ps1` signs `winspaces.exe` and the installer itself when a certificate thumbprint is provided (SHA-256, RFC-3161 timestamp via DigiCert):

```powershell
.\scripts\new-dev-cert.ps1                          # one-time: self-signed dev cert
$env:WINSPACES_SIGN_THUMBPRINT = '<thumbprint>'
.\dev dist
```

- **Dev cert** (`CN=WinSpaces Dev Signing`, CurrentUser\My): proves the signing plumbing; `Get-AuthenticodeSignature` reports the chain as untrusted (self-signed), which is expected.
- **Production**: requires a purchased **OV or EV code-signing certificate** — a business/identity-verification purchase that cannot be automated here. Until then, SmartScreen will warn on downloads regardless of installer quality. EV removes the SmartScreen reputation ramp-up entirely; OV earns reputation over download volume.

## 3. Updates

- **Manual affordance (shipped)**: tray menu → *Check for Updates...* opens `https://github.com/izantech/winspaces/releases/latest` in the browser (`UPDATE_URL` in `main.rs`). The tray header shows the running version (`CARGO_PKG_VERSION`). No network code lives in the daemon.
- **Release flow**: `.\dev release <x.y.z>` on a clean, up-to-date `main` bumps `[workspace.package].version`, moves the *Unreleased* section of `CHANGELOG.md` under the new version, refreshes `Cargo.lock`, commits `chore(release): vx.y.z` and creates the annotated tag; add `-Push` to push both. The pushed tag runs `.github/workflows/release.yml`, which first runs the `ci.yml` gate, then builds the installer on `windows-latest`, checks that the tag matches the packaged version, and attaches the installer plus `SHA256SUMS` to a **draft** GitHub release with generated notes. Publishing is manual. CI signing stays disabled until a production certificate exists (import PFX from a secret, set `WINSPACES_SIGN_THUMBPRINT`).
- **Upgrade in place**: users run the new installer over the old install; §1's upgrade path handles the running daemon.
- **Future** (deliberately not built): an in-app version check would need an HTTP client plus a version endpoint; revisit only if the manual flow proves insufficient.

## 4. Package Managers (winget / cargo)

Both channels wrap the same GitHub release; nothing can be submitted until the draft release from §3 is published. Chocolatey is deliberately not set up.

**winget** (`winget install izantech.WinSpaces`)

- The manifest lives in `microsoft/winget-pkgs` under `manifests/i/izantech/WinSpaces/<ver>/`, installer type `inno`, scope `user`, product code `{7F1FA3E1-4D2B-4E1C-9B7A-2C54A0E63D11}_is1` (the Inno `AppId` plus `_is1`, which is the Uninstall registry key the installer writes). winget adds the silent switches for Inno itself; the post-install launch is `skipifsilent`.
- First version, by hand: `winget install wingetcreate`, then `wingetcreate new https://github.com/izantech/winspaces/releases/download/v<ver>/WinSpaces-Setup-x64-<ver>.exe`; answer the prompts (identifier `izantech.WinSpaces`, publisher `izantech`, license `GPL-3.0-or-later`, homepage `https://winspaces.app`) and let it open the pull request. `winget validate <manifest dir>` and `Tools/SandboxTest.ps1` from the winget-pkgs checkout test it locally.
- Later versions: `.github/workflows/winget.yml` (`vedantmgoyal9/winget-releaser`) opens the pull request when a release is published. It needs the `WINGET_TOKEN` secret (classic PAT, `public_repo` scope) and the package already present in winget-pkgs.
- An unsigned installer is accepted; it only misses the verified-publisher fast path. The SmartScreen story is still §2.

**cargo** (`cargo install winspaces`)

- Builds from source on the user's machine, so it needs the §5 toolchain; ships the bare `winspaces.exe` without shortcuts (autostart is a toggle in the settings window). `cargo install --git https://github.com/izantech/winspaces winspaces` works without anything published.
- crates.io: the four library crates are path dependencies with a version (`[workspace.dependencies]`, kept in lockstep by `dev release`), so publish in dependency order after tagging: `cargo publish -p winspaces-common`, `-p winspaces-win32`, `-p winspaces-core`, `-p winspaces-ui`, `-p winspaces` (`cargo login` first; crates.io allows a burst of five new crates). `winspaces-bench` is `publish = false`. Every release republishes all five.

## 5. Machine Prerequisites (development)

| Tool | Used for | Install |
| :--- | :--- | :--- |
| Rust toolchain | builds | `rustup`; `rust-toolchain.toml` picks the channel and components, `rust-version` in `Cargo.toml` is the floor |
| MSVC x64 desktop toolset + Windows 10/11 SDK | linking (`x86_64-pc-windows-msvc`) | Visual Studio "Desktop development with C++" workload, or Build Tools with the "MSVC x64/x86 build tools" component. `rustc` locates the newest installed Visual Studio through `vswhere`, so that instance must carry the desktop `lib\x64` libraries — a OneCore-only toolset fails to link |
| Inno Setup 6 | `dev dist` | `winget install -e --id JRSoftware.InnoSetup` |
| signtool (Windows SDK) | signing only | ships with the Windows SDK |

## See also

- [`ipc-and-config.md`](ipc-and-config.md) §6 for the elevation posture the installer matches.
- [`user-guide.md`](user-guide.md) §2 for the install, silent-install and portable-mode instructions.
- [`crate-layout.md`](crate-layout.md) §3 for the checks CI runs before packaging.
