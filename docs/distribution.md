# Distribution: Installer, Signing & Updates

How WinSpaces is packaged, signed, and updated. The pipeline entry point is `.\dev dist`, which delegates to `scripts/make-installer.ps1`.

---

## 1. Installer (Inno Setup 6)

`installer/winspaces.iss`, compiled by `scripts/make-installer.ps1` into `dist/WinSpaces-Setup-x64-<version>.exe`. The version is parsed from `crates/winspaces/Cargo.toml` — bump it there; nothing else encodes it.

Design decisions:

- **Per-user install, no UAC** (`PrivilegesRequired=lowest`, installs to `%LOCALAPPDATA%\Programs\WinSpaces`). Matches the non-elevated daemon posture ([`ipc-and-config.md`](ipc-and-config.md) §5). The opt-in elevated scheduled task is *not* an installer feature — power users run `{app}\scripts\install-elevated-autostart.ps1` from an elevated shell.
- **Why Inno, not MSIX**: the daemon manipulates other processes' windows (`SetProp`, DWM cloaks, LL keyboard hook) and autostarts a tray process — all friction points under MSIX containment, and MSIX sideloading requires a trusted signature just to install. Classic setup matches the product; comparable tools (komorebi et al.) ship the same way.
- **Tiny payload, zero runtimes**: the staged payload is `winspaces.exe` (daemon + settings window in one native binary) plus two recovery/autostart scripts — 3 files, ~2 MB installer. Target machines need no runtime of any kind; the settings window ships inside the daemon exe and opens via `winspaces.exe --settings`.
- **Autostart task** (checked by default) writes the same HKCU `Run` value the settings window's autostart toggle manages, so both stay in sync with the installer's choice.
- **Upgrades**: `PrepareToInstall` posts `--exit` to a running daemon and waits for the exe file lock to release — the daemon uncloaks every managed window before files are replaced, so an upgrade can never strand hidden windows.
- **Uninstall**: runs `--exit` (same guarantee), removes the `Run` value, and deletes the elevated scheduled task if one exists.

End-to-end verification procedure (re-run after payload changes): silent install (`/VERYSILENT`), daemon + settings window launch from the install dir on a machine path with no dev runtimes involved, silent uninstall leaves no files, no `Run` value, no processes. Note the uninstaller's `--exit` stops *any* running daemon, so run this when a daemon restart is acceptable.

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
- **Release flow**: push a `v*` tag → `.github/workflows/release.yml` builds the installer on `windows-latest` and attaches it to a **draft** GitHub release with generated notes. Publishing is manual. CI signing stays disabled until a production certificate exists (import PFX from a secret, set `WINSPACES_SIGN_THUMBPRINT`).
- **Upgrade in place**: users run the new installer over the old install; §1's upgrade path handles the running daemon.
- **Future** (deliberately not built): an in-app version check would need an HTTP client plus a version endpoint; revisit only if the manual flow proves insufficient.

## 4. Package-Manager Readiness (winget / Chocolatey — future work)

Nothing is configured yet, but the installer already satisfies what future manifests need:

- **Stable asset URL**: the tag-triggered CI produces `releases/download/v<ver>/WinSpaces-Setup-x64-<ver>.exe`; keep this naming.
- **Silent install**: Inno's `/VERYSILENT /NORESTART` work; the post-install launch is `skipifsilent`.
- **Stable identity**: the `AppId` GUID in `winspaces.iss` is fixed; `VersionInfoVersion` stamps PE version metadata for winget's `AppsAndFeaturesEntries`.
- **Signing**: the SmartScreen/trust story is the OV/EV certificate purchase (§2), which also unblocks a clean winget submission.

When the time comes: winget needs a manifest PR to `microsoft/winget-pkgs` (installer type `inno`); Chocolatey needs a `.nuspec` + `chocolateyinstall.ps1` wrapping the same setup exe with silent args.

## 5. Machine Prerequisites (development)

| Tool | Used for | Install |
| :--- | :--- | :--- |
| Inno Setup 6 | `dev dist` | `winget install -e --id JRSoftware.InnoSetup` |
| signtool (Windows SDK) | signing only | ships with the Windows SDK |
| Rust toolchain | builds | already required by `dev build` |
