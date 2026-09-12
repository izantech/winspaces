# Serves the WinSpaces static documentation & landing site locally.
param(
  [int]$Port = 8338
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

$SCRIPT_DIR = Split-Path -Parent $MyInvocation.MyCommand.Path
$ROOT_DIR = Split-Path -Parent $SCRIPT_DIR
$SITE_DIR = Join-Path $ROOT_DIR "site"

if (-not (Test-Path $SITE_DIR)) {
  Write-Host "[site] ERROR: site directory not found at $SITE_DIR" -ForegroundColor Red
  exit 1
}

# Stop any previous http.server running on this port
Get-CimInstance Win32_Process -Filter "Name = 'python.exe'" -ErrorAction SilentlyContinue |
  Where-Object { $_.CommandLine -like "*http.server*$Port*" } |
  ForEach-Object { Stop-Process -Id $_.ProcessId -Force -ErrorAction SilentlyContinue }

Write-Host "[site] Starting server on http://127.0.0.1:$Port/ ..." -ForegroundColor Cyan

# Launch python process
$proc = Start-Process -FilePath python -ArgumentList "-m","http.server","$Port","--bind","127.0.0.1","--directory",$SITE_DIR -PassThru -WindowStyle Hidden

# Poll until server responds with 200 OK
$ready = $false
for ($attempt = 0; $attempt -lt 20; $attempt++) {
  Start-Sleep -Milliseconds 150
  try {
    $code = curl.exe -s -o NUL -w "%{http_code}" "http://127.0.0.1:$Port/"
    if ($code -eq "200") {
      $ready = $true
      break
    }
  } catch {}
}

if (-not $ready) {
  if (-not $proc.HasExited) { Stop-Process -Id $proc.Id -Force -ErrorAction SilentlyContinue }
  Write-Host "[site] ERROR: Server failed to start on http://127.0.0.1:$Port/" -ForegroundColor Red
  exit 1
}

Write-Host "[site] Serving $SITE_DIR on http://127.0.0.1:$Port/" -ForegroundColor Green
Write-Host "[site] Opening browser... Press Ctrl+C to stop." -ForegroundColor Gray

Start-Process "http://127.0.0.1:$Port/"

try {
  Wait-Process -Id $proc.Id
} finally {
  if (-not $proc.HasExited) {
    Stop-Process -Id $proc.Id -Force -ErrorAction SilentlyContinue
  }
  Write-Host "[site] Server stopped." -ForegroundColor Yellow
}
