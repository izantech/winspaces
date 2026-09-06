# `dev bench` back end: builds and runs winspaces-bench without ever
# touching the daemon exe the user's elevated instance has locked
# (`target\release\winspaces.exe`). See docs\benchmarks.md for what each
# group and scenario measures.
#
#   .\dev bench help
#   .\dev bench static
#   .\dev bench live --quick --scenario idle,switch
#   .\dev bench live --scenario menu -Admin      # menu needs an elevated harness
#   .\dev bench compare base.json new.json --md out.md
#   .\dev bench ab b6c0cea -- --quick            # measure a past commit vs HEAD

param(
  [Parameter(Position = 0)][string]$Command = 'help',
  [Parameter(ValueFromRemainingArguments = $true)][string[]]$Rest,
  [switch]$Admin,
  # Named DebugBuild, not Debug: the [Parameter()] attributes above make this
  # an "advanced" script, so PowerShell auto-adds the common -Debug parameter
  # to it. A same-named switch collides with that and silently breaks
  # Get-Command's parameter metadata, which dev.ps1's Invoke-Script needs to
  # bind -Admin/-Keep by name.
  [switch]$DebugBuild,
  [switch]$Keep
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
$SCRIPT_PATH = $MyInvocation.MyCommand.Path
$SCRIPT_DIR = Split-Path -Parent $SCRIPT_PATH
$ROOT_DIR = Split-Path -Parent $SCRIPT_DIR
Set-Location $ROOT_DIR

function Log($msg) { Write-Host "[bench] $msg" }
function Die($msg) { Write-Host "[bench] ERROR: $msg" -ForegroundColor Red; exit 1 }
function Check-Exit { if ($LASTEXITCODE) { Die "Previous command failed (exit $LASTEXITCODE)" } }

if (-not $Rest) { $Rest = @() }

$HELP = @'
Usage: .\dev bench <command> [options]

Commands (docs\benchmarks.md has what each group and scenario measures):
  micro        pure-logic benchmarks (tiling, matching, layout, config, i18n)
  primitives   the Win32 calls the daemon pays per event
  static       the binary: size, PE sections and imports, dependency count
  live         drive the running daemon and sample its counters per scenario
  all          micro + primitives + static + live (default scenarios)
  smoke        one iteration each, no live group (what `dev check` runs)
  compare <base.json> <new.json>   diff two results, flag regressions (exit 2)
  report <result.json>             render a result's markdown again
  ab <sha>     build <sha> in a throwaway worktree, run `live --own` on it and
               on HEAD back to back, then compare (stops/restarts your daemon
               twice; see docs\benchmarks.md)
  help

Options:
  -Admin       relaunch elevated (UAC prompt); needed for the `menu` scenario,
               which is skipped otherwise (docs\benchmarks.md section 3)
  -DebugBuild  build/run the debug profile instead of release (smoke only)
  -Keep        ab: keep the worktree instead of removing it afterwards

Anything else is passed through to winspaces-bench.exe, for example:
  .\dev bench live --quick --scenario idle,switch,reload
  .\dev bench live --scenario menu -Admin
  .\dev bench ab b6c0cea -- --quick
'@

function Bench-ProfileDir {
  if ($script:DebugBuild) { return 'debug' }
  return 'release'
}

# Builds winspaces-bench only. This never touches target\release\winspaces.exe
# (the daemon exe) or runs any workspace-wide build.
function Build-Bench {
  if ($script:DebugBuild) {
    Log 'cargo build -p winspaces-bench'
    cargo build -p winspaces-bench
  } else {
    Log 'cargo build --release -p winspaces-bench'
    cargo build --release -p winspaces-bench
  }
  Check-Exit
  $exe = Join-Path $ROOT_DIR "target\$(Bench-ProfileDir)\winspaces-bench.exe"
  if (-not (Test-Path $exe)) { Die "Build succeeded but $exe is missing" }
  return $exe
}

function Invoke-Bench([string[]]$Arguments) {
  $exe = Build-Bench
  Log "$exe $($Arguments -join ' ')"
  # Piped through Out-Host: this function's result is assigned by its
  # callers (`$code = Invoke-Bench ...`), and without this the exe's own
  # stdout would be captured into the pipeline right alongside the
  # `return $LASTEXITCODE` below, turning $code into an array instead of a
  # plain exit code.
  & $exe @Arguments | Out-Host
  return $LASTEXITCODE
}

# Relaunches this script elevated for one command, forcing --out to a known
# path so the result can be found again once the elevated console closes.
function Invoke-Elevated([string]$Cmd, [string[]]$Arguments) {
  $resultsDir = Join-Path $ROOT_DIR '.local\bench\results'
  New-Item -ItemType Directory -Force $resultsDir | Out-Null
  $stamp = Get-Date -Format 'yyyyMMdd-HHmmss'
  $outPath = Join-Path $resultsDir "$stamp-admin-$Cmd.json"
  $mdPath = [IO.Path]::ChangeExtension($outPath, 'md')

  $elevatedArgs = @($Cmd) + $Arguments + @('--out', $outPath)
  Log "Relaunching elevated: $Cmd $($Arguments -join ' ') --out $outPath"
  $psArgs = @('-NoProfile', '-File', $SCRIPT_PATH) + $elevatedArgs
  Start-Process powershell -Verb RunAs -ArgumentList $psArgs -Wait

  if (-not (Test-Path $mdPath)) {
    Die "Elevated run did not produce $mdPath (check the elevated console for errors)"
  }
  Log "Result: $outPath"
  Log "Report: $mdPath"
  Get-Content $mdPath -TotalCount 40
}

function Invoke-Ab([string]$Sha, [string[]]$Extra) {
  Log 'WARNING: ab stops and restarts your running daemon twice (once per build) to measure them back to back.'

  $verified = (& git rev-parse --verify $Sha 2>$null)
  Check-Exit
  if (-not $verified) { Die "Not a valid commit-ish: $Sha" }
  $sha7 = $verified.Trim().Substring(0, 7)
  $headSha = (& git rev-parse --short=7 HEAD).Trim()

  $worktree = Join-Path $ROOT_DIR ".local\bench\worktrees\$sha7"
  $resultsDir = Join-Path $ROOT_DIR '.local\bench\results'
  New-Item -ItemType Directory -Force $resultsDir | Out-Null
  $stamp = Get-Date -Format 'yyyyMMdd-HHmmss'

  if (Test-Path $worktree) {
    Log "Reusing existing worktree $worktree"
  } else {
    Log "git worktree add $worktree $Sha"
    git worktree add $worktree $Sha
    Check-Exit
  }

  $bench = Build-Bench
  $compareExitCode = 0

  try {
    Log 'cargo build --release -p winspaces (in the worktree; it has its own target)'
    Push-Location $worktree
    try {
      cargo build --release -p winspaces
      Check-Exit
    } finally {
      Pop-Location
    }

    $baseExe = Join-Path $worktree 'target\release\winspaces.exe'
    $baseOut = Join-Path $resultsDir "$stamp-$sha7-base.json"
    $newOut = Join-Path $resultsDir "$stamp-$headSha-new.json"
    $compareOut = Join-Path $resultsDir "$stamp-compare.md"

    Log "live --own against $sha7"
    & $bench live --own --exe $baseExe --out $baseOut @Extra
    Check-Exit

    Log "live --own against HEAD ($headSha)"
    & $bench live --own --out $newOut @Extra
    Check-Exit

    Log 'compare'
    & $bench compare $baseOut $newOut --md $compareOut
    $compareExitCode = $LASTEXITCODE
    Get-Content $compareOut
  } finally {
    if ($script:Keep) {
      Log "Keeping worktree at $worktree (-Keep)"
    } else {
      Log "Removing worktree $worktree"
      git worktree remove --force $worktree
    }
  }

  exit $compareExitCode
}

switch ($Command) {
  'help' { Write-Host $HELP; exit 0 }
  { $_ -in 'micro', 'primitives', 'static', 'live', 'all', 'smoke', 'report' } {
    if ($Admin) {
      Invoke-Elevated $Command $Rest
      exit 0
    }
    $code = Invoke-Bench (@($Command) + $Rest)
    exit $code
  }
  'compare' {
    $code = Invoke-Bench (@($Command) + $Rest)
    exit $code
  }
  'ab' {
    if ($Rest.Count -lt 1) { Die 'ab needs a commit-ish, e.g. .\dev bench ab b6c0cea' }
    $sha = $Rest[0]
    $extra = if ($Rest.Count -gt 1) { $Rest[1..($Rest.Count - 1)] } else { @() }
    Invoke-Ab $sha $extra
  }
  default {
    Write-Host $HELP
    Die "Unknown command: $Command"
  }
}
