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
function Invoke-Script {
  param([string]$Name, [string[]]$Rest = @())
  $path = Join-Path $ROOT_DIR "scripts\$Name"
  if (-not (Test-Path $path)) {
    Die "Script not found: $path"
  }
  & $path @Rest
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
  clippy     cargo clippy --workspace -- -D warnings
  check      fmt --check + clippy + test
  clean      cargo clean
  all        check + build
  dist       build the distributable installer (dist\WinSpaces-Setup-x64-<ver>.exe)
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

  $cargoCommands = @('build', 'run', 'test', 'fmt', 'clippy', 'check', 'clean', 'all')

  if ($cargoCommands -contains $cmd) {
    Invoke-Script 'cargo-tools.ps1' (@($cmd) + $rest)
  } elseif ($cmd -eq 'dist') {
    Invoke-Script 'make-installer.ps1' $rest
  } elseif ($cmd -eq 'recover') {
    Invoke-Script 'recover-windows.ps1' $rest
  } else {
    Usage
    Die "Unknown command: $cmd"
  }
}

Main $args
