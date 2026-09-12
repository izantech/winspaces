# Cuts a release from a clean, up-to-date `main`: bumps [workspace.package]
# version in Cargo.toml, rolls the Unreleased section of CHANGELOG.md under the
# new version, refreshes Cargo.lock, commits, and creates the annotated tag that
# .github/workflows/release.yml turns into a draft release.
#
#   .\dev release 0.2.0          # bump + changelog + commit + tag v0.2.0, locally
#   .\dev release 0.2.0 -Push    # ...and push main and the tag (starts release.yml)
#
# Publishing the draft release stays a manual step (docs/distribution.md section 3).

param(
  [Parameter(Mandatory = $true, Position = 0)][string]$Version,
  [switch]$Push
)

$ErrorActionPreference = 'Stop'
$SCRIPT_DIR = Split-Path -Parent $MyInvocation.MyCommand.Path
$ROOT_DIR = Split-Path -Parent $SCRIPT_DIR
Set-Location $ROOT_DIR

function Log($msg) { Write-Host "[release] $msg" }
function Die($msg) { Write-Host "[release] ERROR: $msg" -ForegroundColor Red; exit 1 }
function Check-Exit { if ($LASTEXITCODE) { Die "Previous command failed (exit $LASTEXITCODE)" } }

$REPO_URL = 'https://github.com/izantech/winspaces'
$utf8 = New-Object System.Text.UTF8Encoding($false)

if ($Version -notmatch '^\d+\.\d+\.\d+(-[0-9A-Za-z.-]+)?$') {
  Die "'$Version' is not a semantic version (x.y.z or x.y.z-pre)"
}
$tag = "v$Version"

# --- Preconditions -------------------------------------------------------------
$branch = (& git rev-parse --abbrev-ref HEAD).Trim()
if ($branch -ne 'main') { Die "Release from main (current branch: $branch)" }
if (@(& git status --porcelain).Count) { Die 'The working tree is not clean' }
& git fetch --quiet origin main
Check-Exit
$behind = (& git rev-list --count 'HEAD..origin/main').Trim()
if ($behind -ne '0') { Die "main is $behind commit(s) behind origin/main; pull first" }
if (@(& git tag --list $tag).Count) { Die "Tag $tag already exists" }

$cargoTomlPath = Join-Path $ROOT_DIR 'Cargo.toml'
$manifest = [IO.File]::ReadAllText($cargoTomlPath)
$versionMatch = [regex]::Match($manifest, '(?ms)^\[workspace\.package\].*?^version\s*=\s*"([^"]+)"')
if (-not $versionMatch.Success) { Die 'Could not find the [workspace.package] version in Cargo.toml' }
$previous = $versionMatch.Groups[1].Value
if ($previous -eq $Version) { Die "Cargo.toml already carries $Version" }

$changelogPath = Join-Path $ROOT_DIR 'CHANGELOG.md'
$changelog = [IO.File]::ReadAllText($changelogPath)
$unreleased = [regex]::Match($changelog, '(?ms)^## \[Unreleased\]\s*$(.*?)(?=^## \[|\z)')
if (-not $unreleased.Success) { Die 'CHANGELOG.md has no "## [Unreleased]" section' }
if ($unreleased.Groups[1].Value -notmatch '(?m)^### ') { Die 'The Unreleased section of CHANGELOG.md is empty' }

# --- Apply ---------------------------------------------------------------------
$date = Get-Date -Format 'yyyy-MM-dd'
Log "Bumping $previous -> $Version"
$group = $versionMatch.Groups[1]
$manifest = $manifest.Substring(0, $group.Index) + $Version + $manifest.Substring($group.Index + $group.Length)
# The path dependencies in [workspace.dependencies] carry the same version so
# the crates can be published; keep them in lockstep.
$manifest = [regex]::Replace($manifest, '(?m)^(winspaces-[a-z0-9]+ = \{ path = "[^"]+", version = ")[^"]+"', ('${1}' + $Version + '"'))
[IO.File]::WriteAllText($cargoTomlPath, $manifest, $utf8)

$heading = "## [Unreleased]`n`n## [$Version] - $date"
$changelog = (New-Object regex '(?m)^## \[Unreleased\][ \t]*$').Replace($changelog, $heading, 1)

# Comparison links at the bottom of the file, keep-a-changelog style.
$hasPreviousTag = [bool](@(& git tag --list "v$previous").Count)
$unreleasedLink = "[Unreleased]: $REPO_URL/compare/$tag...HEAD"
$versionLink = if ($hasPreviousTag) { "[$Version]: $REPO_URL/compare/v$previous...$tag" } else { "[$Version]: $REPO_URL/releases/tag/$tag" }
$linkPattern = New-Object regex '(?m)^\[Unreleased\]: .*$'
if ($linkPattern.IsMatch($changelog)) {
  $changelog = $linkPattern.Replace($changelog, ($unreleasedLink + "`n" + $versionLink), 1)
} else {
  $changelog = $changelog.TrimEnd() + "`n`n" + $unreleasedLink + "`n" + $versionLink + "`n"
}
[IO.File]::WriteAllText($changelogPath, $changelog, $utf8)

Log 'cargo check --workspace (refreshes Cargo.lock)'
cargo check --workspace
Check-Exit
$metadataJson = & cargo metadata --no-deps --format-version 1 | Out-String
Check-Exit
$package = ($metadataJson | ConvertFrom-Json).packages | Where-Object { $_.name -eq 'winspaces' } | Select-Object -First 1
if ($package.version -ne $Version) { Die "cargo reports $($package.version) after the bump" }

& git add Cargo.toml Cargo.lock CHANGELOG.md
Check-Exit
& git commit --quiet -m "chore(release): $tag"
Check-Exit
& git tag -a $tag -m "WinSpaces $Version"
Check-Exit
Log "Committed and tagged $tag"

if ($Push) {
  & git push origin main
  Check-Exit
  & git push origin $tag
  Check-Exit
  Log 'Pushed; release.yml is building the installer for a draft release'
} else {
  Log "Not pushed. When ready: git push origin main $tag"
}
