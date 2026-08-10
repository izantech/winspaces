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

$DOTNET_BIN = Join-Path $env:LocalAppData "Microsoft\dotnet\dotnet.exe"
if (-not (Test-Path $DOTNET_BIN)) {
  $DOTNET_BIN = "dotnet"
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
    foreach ($p in $procs) {
      try {
        Stop-Process -Id $p.Id -Force -ErrorAction SilentlyContinue
      } catch {}
    }
    Start-Sleep -Milliseconds 200
  }
}

function Cmd-Build {
  Stop-Process -Name "WinSpaces.Gui" -Force -ErrorAction SilentlyContinue
  Stop-Process -Name "winspaces" -Force -ErrorAction SilentlyContinue
  $outDir = if ($script:Configuration -eq 'release') { Join-Path $ROOT_DIR "target\release" } else { Join-Path $ROOT_DIR "target\debug" }

  if ($script:Configuration -eq 'release') {
    Log "cargo build --release --workspace (Rust Core)"
    cargo build --release --workspace
    Check-Exit
    Log "dotnet publish gui/WinSpaces.Gui -c Release -r win-x64 -p:Platform=x64 (C# WinUI 3 GUI)"
    & $DOTNET_BIN publish gui/WinSpaces.Gui/WinSpaces.Gui.csproj -c Release -r win-x64 -p:Platform=x64 -o $outDir
    Check-Exit
  } else {
    Log "cargo build --workspace (Rust Core)"
    cargo build --workspace
    Check-Exit
    Log "dotnet publish gui/WinSpaces.Gui -c Debug -r win-x64 -p:Platform=x64 (C# WinUI 3 GUI)"
    & $DOTNET_BIN publish gui/WinSpaces.Gui/WinSpaces.Gui.csproj -c Debug -r win-x64 -p:Platform=x64 -o $outDir
    Check-Exit
  }
}

function Cmd-Run {
  $targetPkg = "winspaces-daemon"
  $pass = @()

  foreach ($item in $script:Passthrough) {
    if ($item -in 'gui', 'winspaces-gui', '--gui') {
      $targetPkg = "winspaces-gui"
    } elseif ($item -in 'daemon', 'winspaces-daemon', '--daemon') {
      $targetPkg = "winspaces-daemon"
    } else {
      $pass += $item
    }
  }

  $configDir = if ($script:Configuration -eq 'release') { "Release" } else { "Debug" }

  if ($targetPkg -eq "winspaces-gui") {
    Stop-ExistingProcess "WinSpaces.Gui"
    Stop-ExistingProcess "winspaces-gui"
    Cmd-Build

    $rustConfigDir = if ($script:Configuration -eq 'release') { "release" } else { "debug" }
    $exePath = Join-Path $ROOT_DIR "target\$rustConfigDir\WinSpaces.Gui.exe"
    if (-not (Test-Path $exePath)) {
      $exePath = Join-Path $ROOT_DIR "gui\WinSpaces.Gui\bin\x64\$configDir\net8.0-windows10.0.22621.0\win-x64\WinSpaces.Gui.exe"
    }
    if (-not (Test-Path $exePath)) {
      $exePath = Join-Path $ROOT_DIR "gui\WinSpaces.Gui\bin\$configDir\net8.0-windows\WinSpaces.Gui.exe"
    }
    if (-not (Test-Path $exePath)) {
      Die "C# GUI Executable not found at: $exePath"
    }

    Log "Launching native C# Windows 11 GUI asynchronously ($exePath)..."
    if ($pass.Count -gt 0) {
      $null = Start-Process -FilePath $exePath -ArgumentList $pass
    } else {
      $null = Start-Process -FilePath $exePath
    }
    Log "Launched C# GUI successfully. Terminal is free."
  } else {
    Stop-ExistingProcess "winspaces"
    Cmd-Build

    $rustConfigDir = if ($script:Configuration -eq 'release') { "release" } else { "debug" }
    $exePath = Join-Path $ROOT_DIR "target\$rustConfigDir\winspaces.exe"
    if (-not (Test-Path $exePath)) {
      Die "Daemon Executable not found at: $exePath"
    }

    Log "Launching Rust daemon asynchronously ($exePath)..."
    if ($pass.Count -gt 0) {
      $null = Start-Process -FilePath $exePath -ArgumentList $pass
    } else {
      $null = Start-Process -FilePath $exePath
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
  Log "dotnet clean gui/WinSpaces.Gui"
  & $DOTNET_BIN clean gui/WinSpaces.Gui/WinSpaces.Gui.csproj
}

function Cmd-All {
  Cmd-Check
  Cmd-Build
}

function Usage {
  Write-Host @"
Usage: dev <command> [options]

Commands:
  build   Builds Rust daemon (cargo) + C# GUI (dotnet)
  run     Runs daemon or C# GUI asynchronously
          Examples:
            dev run             -> Runs Rust daemon as Admin asynchronously
            dev run gui         -> Runs native C# Windows 11 GUI configurator
            dev run --release   -> Runs daemon (release)
            dev run gui --release -> Runs C# GUI (release)
  test    cargo test --workspace
  fmt     cargo fmt --all
  clippy  cargo clippy --workspace -- -D warnings
  check   fmt --check + clippy + test
  clean   cargo clean + dotnet clean
  all     check + build
  help    Show this help

Options:
  --release  Optimized release profile
  --debug    Debug profile (default)
"@
}

function Main {
  param([string[]]$Arguments)
  $cmd = if ($Arguments.Count -gt 0) { $Arguments[0] } else { 'help' }

  $script:Configuration = 'debug'
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
