$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

# scripts\ is one level below the repo root.
$SCRIPT_DIR = Split-Path -Parent $MyInvocation.MyCommand.Path
$ROOT_DIR = Split-Path -Parent $SCRIPT_DIR
Set-Location $ROOT_DIR

function Log($msg) {
  Write-Host "[dev] $msg"
}

function Die($msg) {
  Write-Host "[dev] ERROR: $msg" -ForegroundColor Red
  exit 1
}

# Abort the current composite flow if the previous native command failed.
function Check-Exit {
  if ($LASTEXITCODE) {
    Die "Previous command failed (exit $LASTEXITCODE)"
  }
}

function Get-TargetExe {
  $profileDir = if ($script:Configuration -eq 'release') { 'release' } else { 'debug' }
  return Join-Path $ROOT_DIR "target\$profileDir\winspaces.exe"
}

# A running exe keeps its image file locked; that lock is the one signal that
# works for an elevated daemon too (Get-Process/CIM cannot read its path).
function Test-ExeUnlocked([string]$path) {
  try {
    $stream = [IO.File]::Open($path, 'Open', 'ReadWrite', 'None')
    $stream.Close()
    return $true
  } catch {
    return $false
  }
}

# Frees this profile's target exe before a build replaces it. Always graceful:
# --exit uncloaks every managed window before the daemon goes away. Never
# force-kills (that strands cloaked windows) and never touches a daemon that
# runs from anywhere else, such as an installed copy.
function Stop-RepoDaemon {
  $exePath = Get-TargetExe
  if (-not (Test-Path $exePath) -or (Test-ExeUnlocked $exePath)) { return }
  Log "Stopping the daemon running from $exePath (--exit)..."
  & $exePath --exit
  $deadline = (Get-Date).AddSeconds(10)
  while (-not (Test-ExeUnlocked $exePath) -and (Get-Date) -lt $deadline) {
    Start-Sleep -Milliseconds 250
  }
  if (-not (Test-ExeUnlocked $exePath)) {
    Die "$exePath is still in use. If that daemon is elevated, run 'winspaces.exe --exit' from an elevated terminal (or Exit from its tray menu), then retry."
  }
}

function Cmd-Build {
  Stop-RepoDaemon

  if ($script:Configuration -eq 'release') {
    Log "cargo build --release --workspace"
    cargo build --release --workspace
    Check-Exit
  } else {
    Log "cargo build --workspace"
    cargo build --workspace
    Check-Exit
  }
}

function Cmd-Run {
  $target = "daemon"
  $pass = @()

  foreach ($item in $script:Passthrough) {
    if ($item -in 'settings', 'gui', '--settings', '--gui') {
      $target = "settings"
    } elseif ($item -in 'daemon', 'winspaces', '--daemon') {
      $target = "daemon"
    } elseif ($item -in '--admin', '-admin', 'admin') {
      $script:AsAdmin = $true
    } else {
      $pass += $item
    }
  }

  Cmd-Build

  $exePath = Get-TargetExe
  if (-not (Test-Path $exePath)) {
    Die "Daemon Executable not found at: $exePath"
  }

  $adminLabel = if ($script:AsAdmin) { " (elevated / RunAs)" } else { "" }

  if ($target -eq "settings") {
    Log "Launching settings window asynchronously$adminLabel ($exePath --settings)..."
    $argsToPass = @('--settings') + $pass
    if ($script:AsAdmin) {
      $null = Start-Process -FilePath $exePath -ArgumentList $argsToPass -Verb RunAs
    } else {
      $null = Start-Process -FilePath $exePath -ArgumentList $argsToPass
    }
    Log "Launched settings window successfully. Terminal is free."
  } else {
    Log "Launching Rust daemon asynchronously$adminLabel ($exePath)..."
    if ($script:AsAdmin) {
      if ($pass.Count -gt 0) {
        $null = Start-Process -FilePath $exePath -ArgumentList $pass -Verb RunAs
      } else {
        $null = Start-Process -FilePath $exePath -Verb RunAs
      }
    } else {
      if ($pass.Count -gt 0) {
        $null = Start-Process -FilePath $exePath -ArgumentList $pass
      } else {
        $null = Start-Process -FilePath $exePath
      }
    }
    Log "Launched daemon successfully. Terminal is free."
  }
}

function Cmd-Test {
  $pass = $script:Passthrough
  Log "cargo test --workspace"
  if ($pass.Count -gt 0) { cargo test --workspace @pass } else { cargo test --workspace }
}

function Cmd-Fmt {
  $pass = $script:Passthrough
  Log "cargo fmt --all"
  if ($pass.Count -gt 0) { cargo fmt --all @pass } else { cargo fmt --all }
}

function Cmd-Clippy {
  Log "cargo clippy --workspace --all-targets -- -D warnings"
  cargo clippy --workspace --all-targets -- -D warnings
}

# Every crate must also build on its own: a workspace build unifies windows-sys
# features across members and hides a missing declaration (see
# docs/crate-layout.md section 3). The feature script catches what `-p` cannot.
function Cmd-Features {
  # Array splatting passes "-Quiet" positionally, so bind the switch explicitly.
  $quiet = $script:Passthrough -contains '-Quiet'
  Log "scripts\check-features.ps1"
  & (Join-Path $ROOT_DIR 'scripts\check-features.ps1') -Quiet:$quiet
}

function Cmd-Check {
  Log "cargo fmt --all --check"
  cargo fmt --all -- --check
  Check-Exit
  Log "cargo clippy --workspace --all-targets -- -D warnings"
  cargo clippy --workspace --all-targets -- -D warnings
  Check-Exit
  Log "cargo test --workspace"
  cargo test --workspace
  Check-Exit
  foreach ($crate in 'winspaces-common', 'winspaces-win32', 'winspaces-core', 'winspaces-ui', 'winspaces') {
    Log "cargo check -p $crate"
    cargo check -p $crate
    Check-Exit
  }
  Log "scripts\check-features.ps1 -Quiet"
  & (Join-Path $ROOT_DIR 'scripts\check-features.ps1') -Quiet
  Check-Exit
}

function Cmd-Clean {
  Log "cargo clean"
  cargo clean
}

function Cmd-All {
  Cmd-Check
  Cmd-Build
}

# The help text lives in dev.ps1 only; this script is its cargo back end.
function Main {
  param([string[]]$Arguments)
  $cmd = if ($Arguments.Count -gt 0) { $Arguments[0] } else { 'help' }

  $script:Configuration = 'debug'
  $script:AsAdmin = $false
  $script:Passthrough = @()
  $parsingFlags = $true
  $pass = @()
  if ($Arguments.Count -gt 1) {
    foreach ($a in $Arguments[1..($Arguments.Count - 1)]) {
      if (-not $parsingFlags) {
        $pass += $a
      } elseif ($a -eq '--release') {
        $script:Configuration = 'release'
      } elseif ($a -eq '--debug') {
        $script:Configuration = 'debug'
      } elseif ($a -in '--admin', '-admin') {
        $script:AsAdmin = $true
      } elseif ($a -eq '--') {
        $parsingFlags = $false
      } else {
        $pass += $a
      }
    }
  }
  $script:Passthrough = $pass

  switch ($cmd) {
    'build'  { Cmd-Build }
    'run'    { Cmd-Run }
    'test'   { Cmd-Test }
    'fmt'    { Cmd-Fmt }
    'clippy' { Cmd-Clippy }
    'features' { Cmd-Features }
    'check'  { Cmd-Check }
    'clean'  { Cmd-Clean }
    'all'    { Cmd-All }
    default  { Die "Unknown command: $cmd (see .\dev help)" }
  }
}

Main $args
