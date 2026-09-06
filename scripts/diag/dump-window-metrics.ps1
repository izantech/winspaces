# Dumps raw Win32 metrics for every top-level application window, independent
# of the daemon: GetWindowRect, DWM extended frame bounds, WINDOWPLACEMENT and
# the monitor each window sits on. Use it to compare what the daemon believes
# (`winspaces.exe --dump`) with what Windows reports.
#
#   .\scripts\diag\dump-window-metrics.ps1                  # every window, to the console
#   .\scripts\diag\dump-window-metrics.ps1 -Filter notepad  # title/class substring
#   .\scripts\diag\dump-window-metrics.ps1 -OutFile dump.txt
#
# Compiles dump_app.cs with the .NET Framework C# compiler on first use (and
# whenever the source is newer than the exe); the exe is git-ignored.

[CmdletBinding()]
param(
    [string]$Filter = "",
    [string]$OutFile = ""
)

$ErrorActionPreference = 'Stop'
$scriptDir = Split-Path -Parent $MyInvocation.MyCommand.Path
$csFile = Join-Path $scriptDir "dump_app.cs"
$exeFile = Join-Path $scriptDir "dump_app.exe"

if (-not (Test-Path $exeFile) -or (Get-Item $csFile).LastWriteTime -gt (Get-Item $exeFile).LastWriteTime) {
    $csc = Get-ChildItem "$env:WINDIR\Microsoft.NET\Framework64\v4*\csc.exe" -ErrorAction SilentlyContinue |
        Sort-Object FullName | Select-Object -Last 1 -ExpandProperty FullName
    if (-not $csc) { throw "csc.exe (.NET Framework 4.x) not found under $env:WINDIR\Microsoft.NET\Framework64" }
    & $csc /nologo /platform:x64 /out:"$exeFile" "$csFile" | Out-Null
    if ($LASTEXITCODE) { throw "csc.exe failed (exit $LASTEXITCODE)" }
}

if ($OutFile) {
    & "$exeFile" "$Filter" "$OutFile"
} else {
    & "$exeFile" "$Filter"
}
