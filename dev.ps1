$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

# This script lives at the repo root.
$ROOT_DIR = Split-Path -Parent $MyInvocation.MyCommand.Path
Set-Location $ROOT_DIR

function Log($msg) {
  Write-Host "[dev] $msg"
}

function Die($msg) {
  Write-Host "[dev] ERROR: $msg" -ForegroundColor Red
  exit 1
}

# Delegate to a helper script under scripts\ and propagate its exit code
# (native-command failure or explicit exit) back to the caller.
#
# Splatting a plain array passes every element positionally, so a "-SkipBuild"
# token would never bind as the script's switch. Named arguments are therefore
# rebuilt from the target script's own parameter table and splatted as a
# hashtable; scripts without a param block (cargo-tools.ps1) get the raw tokens,
# "--" included, because they parse $args themselves.
function Invoke-Script {
  param([string]$Name, [string[]]$Rest = @())
  $path = Join-Path $ROOT_DIR "scripts\$Name"
  if (-not (Test-Path $path)) {
    Die "Script not found: $path"
  }
  $parameters = (Get-Command $path).Parameters
  $named = @{}
  $positional = @()
  for ($i = 0; $i -lt $Rest.Count; $i++) {
    $token = $Rest[$i]
    if ($parameters.Count -gt 0 -and $token -eq '--') { continue }
    if ($token -match '^-([A-Za-z][A-Za-z0-9]*)$' -and $parameters.ContainsKey($Matches[1])) {
      $parameter = $parameters[$Matches[1]]
      if ($parameter.SwitchParameter) {
        $named[$parameter.Name] = $true
      } else {
        $i++
        if ($i -ge $Rest.Count) { Die "-$($parameter.Name) needs a value" }
        $named[$parameter.Name] = $Rest[$i]
      }
    } else {
      $positional += $token
    }
  }
  & $path @positional @named
  if ($LASTEXITCODE) { exit $LASTEXITCODE }
}

function Usage {
  Write-Host @"
Usage: .\dev <command> [options]

A dev task runner for WinSpaces (Rust workspace). Commands delegate to
helper scripts under scripts\.

Commands:
  build      cargo build --workspace (winspaces.exe)
  run        run the daemon or the settings window (inherits the terminal's
             integrity level; use --admin for elevated)
             Examples:
               dev run                    # Runs daemon in background tray
               dev run --admin            # Runs daemon with administrator privileges
               dev run settings           # Opens the native settings window
               dev run settings --admin   # Settings window as admin
               dev run --release          # Runs release daemon
               dev run settings --release # Release settings window
  test       cargo test --workspace
  fmt        cargo fmt --all
  clippy     cargo clippy --workspace --all-targets -- -D warnings
  features   check that every crate declares the windows-sys features it uses
             (scripts\check-features.ps1; see docs\crate-layout.md section 3)
  check      fmt --check + clippy + test + cargo check -p <crate> x5 + features
  clean      cargo clean
  all        check + build
  dist       build the distributable installer (dist\WinSpaces-Setup-x64-<ver>.exe)
               dev dist -SkipBuild        # reuse the staged payload in dist\staging
  cert       create (or print) the self-signed dev code-signing certificate
  release    bump [workspace.package].version, roll CHANGELOG.md, commit and tag
               dev release 0.2.0          # local commit + tag v0.2.0
               dev release 0.2.0 -Push    # ...and push main + tag (runs release.yml)
  recover    stop the daemon, then restore hidden/cloaked windows
               (scripts\recover-windows.ps1; the daemon must stop first, or it
                keeps tracking windows it can no longer hide or show)
  help       Show this help (default)

Options (build/run/test/check/all):
  --release  Optimized release profile
  --debug    Debug profile (default)
  --admin    Run with administrator privileges (UAC prompt if not elevated)
  --         Pass remaining args to the program (run/test)
"@
}

function Main {
  param([string[]]$Arguments)
  $cmd = if ($Arguments.Count -gt 0) { $Arguments[0] } else { 'help' }
  $rest = if ($Arguments.Count -gt 1) { $Arguments[1..($Arguments.Count - 1)] } else { @() }

  if ($cmd -in '-h', '--help', 'help') {
    Usage
    return
  }

  $cargoCommands = @('build', 'run', 'test', 'fmt', 'clippy', 'features', 'check', 'clean', 'all')

  if ($cargoCommands -contains $cmd) {
    Invoke-Script 'cargo-tools.ps1' (@($cmd) + $rest)
  } elseif ($cmd -eq 'dist') {
    Invoke-Script 'make-installer.ps1' $rest
  } elseif ($cmd -eq 'cert') {
    Invoke-Script 'new-dev-cert.ps1' $rest
  } elseif ($cmd -eq 'release') {
    Invoke-Script 'release.ps1' $rest
  } elseif ($cmd -eq 'recover') {
    Invoke-Script 'recover-windows.ps1' $rest
  } else {
    Usage
    Die "Unknown command: $cmd"
  }
}

Main $args
