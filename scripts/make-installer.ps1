# Builds the distributable WinSpaces installer.
#
#   .\dev dist                      # build dist\WinSpaces-Setup-x64-<ver>.exe
#   .\dev dist -- -SkipBuild        # reuse existing dist\staging payload
#   $env:WINSPACES_SIGN_THUMBPRINT  # optional: code-sign exes + installer
#
# Pipeline: stop repo daemon gracefully -> cargo build --release ->
# stage payload -> optional signing -> ISCC -> optional installer signing.
#
# The daemon is restarted afterwards if it was running when we started.

param(
  [switch]$SkipBuild,
  [string]$SignThumbprint = $env:WINSPACES_SIGN_THUMBPRINT
)

$ErrorActionPreference = 'Stop'
$SCRIPT_DIR = Split-Path -Parent $MyInvocation.MyCommand.Path
$ROOT_DIR = Split-Path -Parent $SCRIPT_DIR
Set-Location $ROOT_DIR

function Log($msg) { Write-Host "[dist] $msg" }
function Die($msg) { Write-Host "[dist] ERROR: $msg" -ForegroundColor Red; exit 1 }
function Check-Exit { if ($LASTEXITCODE) { Die "Previous command failed (exit $LASTEXITCODE)" } }

# --- Locate tools -----------------------------------------------------------
$ISCC = @(
  "$env:LOCALAPPDATA\Programs\Inno Setup 6\ISCC.exe",
  "${env:ProgramFiles(x86)}\Inno Setup 6\ISCC.exe",
  "$env:ProgramFiles\Inno Setup 6\ISCC.exe"
) | Where-Object { Test-Path $_ } | Select-Object -First 1
if (-not $ISCC) { Die "Inno Setup 6 not found. Install with: winget install -e --id JRSoftware.InnoSetup" }

$SIGNTOOL = $null
if ($SignThumbprint) {
  $SIGNTOOL = Get-ChildItem "${env:ProgramFiles(x86)}\Windows Kits\10\bin\*\x64\signtool.exe" -ErrorAction SilentlyContinue |
    Sort-Object FullName | Select-Object -Last 1 -ExpandProperty FullName
  if (-not $SIGNTOOL) { Die "signtool.exe not found (Windows SDK required for signing)" }
}

# --- Version from the daemon crate ------------------------------------------
$cargoToml = Get-Content (Join-Path $ROOT_DIR 'crates\winspaces\Cargo.toml') -Raw
if ($cargoToml -notmatch '(?m)^version\s*=\s*"([^"]+)"') { Die 'Could not parse version from Cargo.toml' }
$VERSION = $Matches[1]
Log "Packaging WinSpaces v$VERSION"

# --- Stop the repo daemon gracefully (release exe gets rebuilt/locked) ------
$daemonWasRunning = $false
$repoExe = Join-Path $ROOT_DIR 'target\release\winspaces.exe'
$running = Get-Process winspaces -ErrorAction SilentlyContinue |
  Where-Object { $_.Path -eq $repoExe }
if ($running -and -not $SkipBuild) {
  $daemonWasRunning = $true
  Log 'Stopping running daemon gracefully (--exit)...'
  Start-Process $repoExe -ArgumentList '--exit' -Wait
  $deadline = (Get-Date).AddSeconds(10)
  while ((Get-Process winspaces -ErrorAction SilentlyContinue) -and (Get-Date) -lt $deadline) {
    Start-Sleep -Milliseconds 250
  }
}

# --- Build + stage -----------------------------------------------------------
$DIST = Join-Path $ROOT_DIR 'dist'
$STAGE = Join-Path $DIST 'staging'

if (-not $SkipBuild) {
  Log 'cargo build --release --workspace'
  cargo build --release --workspace
  Check-Exit

  if (Test-Path $STAGE) { Remove-Item $STAGE -Recurse -Force }
  New-Item -ItemType Directory -Force $STAGE | Out-Null

  Copy-Item $repoExe $STAGE
  New-Item -ItemType Directory -Force (Join-Path $STAGE 'scripts') | Out-Null
  Copy-Item (Join-Path $ROOT_DIR 'scripts\recover-windows.ps1') (Join-Path $STAGE 'scripts')
  Copy-Item (Join-Path $ROOT_DIR 'scripts\install-elevated-autostart.ps1') (Join-Path $STAGE 'scripts')

  # Debug symbols and doc XML have no place in the payload.
  Get-ChildItem $STAGE -Include '*.pdb', '*.xml' -Recurse | Remove-Item -Force
} elseif (-not (Test-Path (Join-Path $STAGE 'winspaces.exe'))) {
  Die "-SkipBuild given but $STAGE has no staged payload"
}

# --- Optional code signing ---------------------------------------------------
if ($SignThumbprint) {
  Log "Signing staged executable (thumbprint $SignThumbprint)"
  & $SIGNTOOL sign /sha1 $SignThumbprint /fd SHA256 /tr http://timestamp.digicert.com /td SHA256 (Join-Path $STAGE 'winspaces.exe')
  Check-Exit
}

# --- Compile installer -------------------------------------------------------
Log "ISCC installer\winspaces.iss -> dist\"
& $ISCC "/DAppVersion=$VERSION" "/DStageDir=$STAGE" "/O$DIST" (Join-Path $ROOT_DIR 'installer\winspaces.iss')
Check-Exit

$setupExe = Join-Path $DIST "WinSpaces-Setup-x64-$VERSION.exe"
if ($SignThumbprint) {
  Log 'Signing installer'
  & $SIGNTOOL sign /sha1 $SignThumbprint /fd SHA256 /tr http://timestamp.digicert.com /td SHA256 $setupExe
  Check-Exit
}

Log "Installer ready: $setupExe ($('{0:N1}' -f ((Get-Item $setupExe).Length / 1MB)) MB)"

# --- Restart the daemon if we stopped it -------------------------------------
if ($daemonWasRunning) {
  Log 'Restarting daemon...'
  $null = Start-Process $repoExe
}
