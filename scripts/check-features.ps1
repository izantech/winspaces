# Verifies that every crate declares every `windows-sys` feature its own source
# uses. Runs from `dev check` / `dev features` and in CI.
#
# Why a script: a workspace build unifies `windows-sys` features across every
# crate in the graph, and `cargo check -p <crate>` still pulls the crate's path
# dependencies into that graph. So a feature that a *lower* crate enables makes
# an *upper* crate compile even when the upper crate never declared it - the
# omission only surfaces when the lower crate stops needing the feature. This
# script catches the omission directly: the module paths named in the crate's
# source must all be covered by the features its own Cargo.toml declares
# (including what those features imply, e.g. Win32_UI_Controls_Dialogs implies
# Win32_UI_Controls).
#
# It proves "declared covers used", not minimality: a feature can be needed for a
# type that appears only in a signature (RegCreateKeyExW needs Win32_Security
# without any source path naming it), so "declared but not named" is printed as
# information, never as a failure.
#
#   .\dev features            # report every crate
#   .\dev features -- -Quiet  # print only failures

param([switch]$Quiet)

$ErrorActionPreference = 'Stop'
$SCRIPT_DIR = Split-Path -Parent $MyInvocation.MyCommand.Path
$ROOT_DIR = Split-Path -Parent $SCRIPT_DIR
Set-Location $ROOT_DIR

function Log($msg) { Write-Host "[features] $msg" }
function Die($msg) { Write-Host "[features] ERROR: $msg" -ForegroundColor Red; exit 1 }

# --- The real feature list of the windows-sys we depend on -------------------
$metadataJson = & cargo metadata --format-version 1 | Out-String
if ($LASTEXITCODE) { Die 'cargo metadata failed' }
$metadata = $metadataJson | ConvertFrom-Json
$windowsSys = $metadata.packages | Where-Object { $_.name -eq 'windows-sys' } | Select-Object -First 1
if (-not $windowsSys) { Die 'windows-sys is not in the dependency graph' }
$known = @{}
foreach ($name in $windowsSys.features.PSObject.Properties.Name) { $known[$name] = $true }

# --- Helpers -----------------------------------------------------------------

# Split "a, b::{c, d}, e" at top-level commas only.
function Split-TopLevel([string]$s) {
  $parts = @()
  $depth = 0
  $current = ''
  foreach ($ch in $s.ToCharArray()) {
    if ($ch -eq '{') { $depth++ } elseif ($ch -eq '}') { $depth-- }
    if ($ch -eq ',' -and $depth -eq 0) { $parts += $current; $current = ''; continue }
    $current += $ch
  }
  if ($current -ne '') { $parts += $current }
  return $parts
}

# Expand a use-tree node ("a::b::{c, d::{e}}") into full "a::b::c" paths.
function Expand-Node([string]$prefix, [string]$node, $out) {
  $brace = $node.IndexOf('{')
  if ($brace -lt 0) {
    if ($node -ne '' -and $node -ne 'self') { $out.Add($prefix + $node) } else { $out.Add($prefix.TrimEnd(':')) }
    return
  }
  $head = $node.Substring(0, $brace)
  $inner = $node.Substring($brace + 1, $node.Length - $brace - 2)
  foreach ($part in (Split-TopLevel $inner)) {
    if ($part -eq '') { continue }
    Expand-Node ($prefix + $head) $part $out
  }
}

function Expand-UseTree([string]$body) {
  $body = $body -replace '//[^\r\n]*', '' -replace '\s+as\s+\w+', '' -replace '\s+', ''
  $out = New-Object System.Collections.Generic.List[string]
  Expand-Node '' $body $out
  return $out
}

# Map "Win32::A::B::Item" to the deepest module feature ("Win32_A_B"): the last
# segments that are items rather than modules are dropped until a real feature
# name is left.
function Resolve-Feature([string[]]$segments) {
  for ($n = $segments.Count; $n -ge 2; $n--) {
    $candidate = ($segments[0..($n - 1)] -join '_')
    if ($known.ContainsKey($candidate)) { return $candidate }
  }
  return $null
}

# Every feature a declared list enables, following windows-sys's own
# feature-implies-feature table.
function Get-Closure([string[]]$declared) {
  $set = @{}
  $stack = New-Object System.Collections.Stack
  foreach ($d in $declared) { $stack.Push($d) }
  while ($stack.Count) {
    $f = $stack.Pop()
    if ($set.ContainsKey($f)) { continue }
    $set[$f] = $true
    if ($known.ContainsKey($f)) {
      foreach ($dep in $windowsSys.features.$f) { $stack.Push($dep) }
    }
  }
  return $set
}

function Get-Declared([string]$cargoToml) {
  $text = Get-Content $cargoToml -Raw
  $m = [regex]::Match($text, '(?s)windows-sys\s*=\s*\{[^}]*?features\s*=\s*\[(.*?)\]')
  if (-not $m.Success) { return @() }
  return @([regex]::Matches($m.Groups[1].Value, '"([^"]+)"') | ForEach-Object { $_.Groups[1].Value })
}

# feature -> first "path:line" that names it.
function Get-Used([string]$crateDir) {
  $files = @(Get-ChildItem (Join-Path $crateDir 'src') -Recurse -Filter '*.rs')
  $buildRs = Join-Path $crateDir 'build.rs'
  if (Test-Path $buildRs) { $files += Get-Item $buildRs }
  $used = @{}
  foreach ($file in $files) {
    $text = Get-Content $file.FullName -Raw
    $hits = New-Object System.Collections.Generic.List[object]
    foreach ($m in [regex]::Matches($text, '(?s)\buse\s+windows_sys::([^;]+);')) {
      foreach ($p in (Expand-UseTree $m.Groups[1].Value)) { $hits.Add(@{ Path = $p; Index = $m.Index }) }
    }
    foreach ($m in [regex]::Matches($text, '\bwindows_sys::(Win32(?:::[A-Za-z0-9_]+)+)')) {
      $hits.Add(@{ Path = $m.Groups[1].Value; Index = $m.Index })
    }
    foreach ($hit in $hits) {
      $segments = $hit.Path -split '::'
      if ($segments[0] -ne 'Win32') { continue }   # windows_sys::core is unconditional
      $feature = Resolve-Feature $segments
      if ($null -eq $feature) { Die "$($file.FullName): cannot map '$($hit.Path)' to a windows-sys feature" }
      if (-not $used.ContainsKey($feature)) {
        $line = ($text.Substring(0, $hit.Index) -split "`n").Count
        $used[$feature] = "$($file.FullName.Substring($ROOT_DIR.Length + 1)):$line"
      }
    }
  }
  return $used
}

# --- Check every crate --------------------------------------------------------
$failed = $false
foreach ($crate in (Get-ChildItem (Join-Path $ROOT_DIR 'crates') -Directory | Sort-Object Name)) {
  $declared = @(Get-Declared (Join-Path $crate.FullName 'Cargo.toml'))
  $closure = Get-Closure $declared
  $used = Get-Used $crate.FullName
  $missing = @($used.Keys | Where-Object { -not $closure.ContainsKey($_) } | Sort-Object)
  $unnamed = @($declared | Where-Object { -not $used.ContainsKey($_) } | Sort-Object)

  if ($missing.Count) {
    $failed = $true
    Write-Host "[features] $($crate.Name): $($missing.Count) feature(s) used but not declared in crates\$($crate.Name)\Cargo.toml" -ForegroundColor Red
    foreach ($f in $missing) { Write-Host "    $f  (first use: $($used[$f]))" }
  } elseif (-not $Quiet) {
    Log "$($crate.Name): ok ($($used.Count) used, $($declared.Count) declared)"
  }
  if ($unnamed.Count -and -not $Quiet) {
    Log "$($crate.Name): declared but not named in source (may still be needed by a signature): $($unnamed -join ', ')"
  }
}

if ($failed) { Die 'windows-sys feature declarations are incomplete' }
if (-not $Quiet) { Log 'all crates declare the windows-sys features they use' }
