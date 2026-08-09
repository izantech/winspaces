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

function Cmd-Build {
  if ($script:Configuration -eq 'release') {
    Log "cargo build --release"
    cargo build --release
  } else {
    Log "cargo build"
    cargo build
  }
}

function Cmd-Run {
  $pass = $script:Passthrough
  if ($script:Configuration -eq 'release') {
    Log "cargo run --release"
    if ($pass.Count -gt 0) { cargo run --release -- @pass } else { cargo run --release }
  } else {
    Log "cargo run"
    if ($pass.Count -gt 0) { cargo run -- @pass } else { cargo run }
  }
}

function Cmd-Test {
  $pass = $script:Passthrough
  Log "cargo test"
  if ($pass.Count -gt 0) { cargo test @pass } else { cargo test }
}

function Cmd-Fmt {
  $pass = $script:Passthrough
  Log "cargo fmt"
  if ($pass.Count -gt 0) { cargo fmt @pass } else { cargo fmt }
}

function Cmd-Clippy {
  Log "cargo clippy -- -D warnings"
  cargo clippy -- -D warnings
}

function Cmd-Check {
  Log "cargo fmt --check"
  cargo fmt --check
  Check-Exit
  Log "cargo clippy -- -D warnings"
  cargo clippy -- -D warnings
  Check-Exit
  Log "cargo test"
  cargo test
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
Usage: dev <cargo-command> [options]

Commands:
  build   cargo build
  run     cargo run [-- <args>]
  test    cargo test [test-filter]
  fmt     cargo fmt
  clippy  cargo clippy -- -D warnings
  check   fmt --check + clippy + test
  clean   cargo clean
  all     check + build
  help    Show this help

Options:
  --release  Optimized release profile
  --debug    Debug profile (default)
  --         Pass remaining args to the program (run/test)
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
    default  { Usage; Die "Unknown cargo command: $cmd" }
  }
}

Main $args
