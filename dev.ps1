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
  build      cargo build --workspace + dotnet publish (winspaces.exe + WinSpaces.Gui.exe)
  run        run the daemon or the C# WinUI 3 GUI
             Examples:
               dev run               # Runs daemon in background tray
               dev run gui           # Runs modern GUI configurator
               dev run --release     # Runs release daemon
               dev run gui --release # Runs release GUI
  test       cargo test --workspace
  fmt        cargo fmt --all
  clippy     cargo clippy --workspace -- -D warnings
  check      fmt --check + clippy + test
  clean      cargo clean
  all        check + build
  recover    run scripts\recover-windows.ps1 to restore hidden windows
  help       Show this help (default)

Options (build/run/test/check/all):
  --release  Optimized release profile
  --debug    Debug profile (default)
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
  } elseif ($cmd -eq 'recover') {
    Invoke-Script 'recover-windows.ps1'
  } else {
    Usage
    Die "Unknown command: $cmd"
  }
}

Main $args
