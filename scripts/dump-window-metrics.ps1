# dump-window-metrics.ps1
# WinSpaces Diagnostic Tool: Dumps detailed Win32 metrics for all top-level application windows.

[CmdletBinding()]
param(
    [string]$Filter = "",
    [string]$OutFile = ""
)

$scriptDir = Split-Path -Parent $MyInvocation.MyCommand.Path
$csFile = Join-Path $scriptDir "dump_app.cs"
$exeFile = Join-Path $scriptDir "dump_app.exe"

if (-not (Test-Path $exeFile) -or (Get-Item $csFile).LastWriteTime -gt (Get-Item $exeFile).LastWriteTime) {
    $csc = "C:\Windows\Microsoft.NET\Framework64\v4.0.30319\csc.exe"
    & $csc /platform:x64 /out:"$exeFile" "$csFile" | Out-Null
}

if ($OutFile) {
    & "$exeFile" "$Filter" "$OutFile"
} else {
    & "$exeFile" "$Filter"
}
