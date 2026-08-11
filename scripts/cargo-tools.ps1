$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

# scripts\ is one level below the repo root.
$SCRIPT_DIR = Split-Path -Parent $MyInvocation.MyCommand.Path
$ROOT_DIR = Split-Path -Parent $SCRIPT_DIR
Set-Location $ROOT_DIR

$msvcLib18 = "C:\Program Files\Microsoft Visual Studio\18\Community\VC\Tools\MSVC\14.51.36231\lib\onecore\x64"
$msvcLib22 = "C:\Program Files\Microsoft Visual Studio\2022\Community\VC\Tools\MSVC\14.44.35103\lib\x64"
$ucrtLib = "C:\Program Files (x86)\Windows Kits\10\Lib\10.0.26100.0\ucrt\x64"
$umLib = "C:\Program Files (x86)\Windows Kits\10\Lib\10.0.26100.0\um\x64"

$validPaths = @($msvcLib18, $msvcLib22, $ucrtLib, $umLib) | Where-Object { Test-Path $_ }
if ($validPaths.Count -gt 0) {
  $env:LIB = ($validPaths -join ";") + ";" + $env:LIB
}

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

function Stop-ExistingProcess {
  param([string]$ProcessName)
  $procs = Get-Process -Name $ProcessName -ErrorAction SilentlyContinue
  if ($procs) {
    Log "Stopping existing running instance(s) of $ProcessName..."
    $rustConfigDir = if ($script:Configuration -eq 'release') { "release" } else { "debug" }
    $exePath = Join-Path $ROOT_DIR "target\$rustConfigDir\winspaces.exe"
    if (Test-Path $exePath) {
      try {
        & $exePath --exit 2>$null
        Start-Sleep -Milliseconds 150
      } catch {}
    }
    foreach ($p in $procs) {
      try {
        Stop-Process -Id $p.Id -Force -ErrorAction SilentlyContinue
      } catch {}
    }
    Start-Sleep -Milliseconds 200
  }
}

function Cmd-Build {
  Stop-Process -Name "winspaces" -Force -ErrorAction SilentlyContinue

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
    } elseif ($item -in 'daemon', 'winspaces-daemon', '--daemon') {
      $target = "daemon"
    } elseif ($item -in '--admin', '-admin', 'admin') {
      $script:AsAdmin = $true
    } else {
      $pass += $item
    }
  }

  if ($target -eq "daemon") {
    Stop-ExistingProcess "winspaces"
  }
  Cmd-Build

  $rustConfigDir = if ($script:Configuration -eq 'release') { "release" } else { "debug" }
  $exePath = Join-Path $ROOT_DIR "target\$rustConfigDir\winspaces.exe"
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
  Log "cargo clippy --workspace -- -D warnings"
  cargo clippy --workspace -- -D warnings
}

function Cmd-Check {
  Log "cargo fmt --all --check"
  cargo fmt --all -- --check
  Check-Exit
  Log "cargo clippy --workspace -- -D warnings"
  cargo clippy --workspace -- -D warnings
  Check-Exit
  Log "cargo test --workspace"
  cargo test --workspace
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

function Usage {
  Write-Host @"
Usage: dev <command> [options]

Commands:
  build   Builds the Rust workspace (daemon + settings window)
  run     Runs the daemon or settings window asynchronously
          Examples:
            dev run                    -> Runs daemon asynchronously
            dev run --admin            -> Runs daemon with administrator privileges
            dev run settings           # Opens the native settings window
            dev run settings --admin   # Settings window as admin
            dev run --release          -> Runs daemon (release)
            dev run settings --release -> Settings window (release)
  test    cargo test --workspace
  fmt     cargo fmt --all
  clippy  cargo clippy --workspace -- -D warnings
  check   fmt --check + clippy + test
  clean   cargo clean
  all     check + build
  help    Show this help

Options:
  --release  Optimized release profile
  --debug    Debug profile (default)
  --admin    Run with administrator privileges (UAC prompt if not elevated)
"@
}

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
    'check'  { Cmd-Check }
    'clean'  { Cmd-Clean }
    'all'    { Cmd-All }
    'help'   { Usage }
    default  { Usage; Die "Unknown command: $cmd" }
  }
}

Main $args
