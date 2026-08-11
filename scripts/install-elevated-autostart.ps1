# Opt-in: run the WinSpaces daemon elevated at logon WITHOUT a UAC prompt,
# via a scheduled task with highest run level.
#
# The default (and recommended) posture is NON-elevated autostart through the
# HKCU Run key managed by the GUI's autostart toggle. Install this task only
# if you need WinSpaces to manage windows of elevated applications
# (see docs/ipc-and-config.md, "Elevation Posture").
#
# Usage (from an elevated PowerShell):
#   .\scripts\install-elevated-autostart.ps1                # install for current user
#   .\scripts\install-elevated-autostart.ps1 -ExePath <path> # custom daemon location
#   .\scripts\install-elevated-autostart.ps1 -Remove        # uninstall the task
#
# Installing removes the HKCU Run entry so the daemon does not start twice
# (the daemon also carries a single-instance guard as a backstop).

param(
  [string]$ExePath,
  [switch]$Remove
)

$ErrorActionPreference = 'Stop'
$TASK_NAME = 'WinSpaces Daemon (Elevated)'
$RUN_KEY = 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Run'
$RUN_VALUE = 'WinSpaces'

$principal = [Security.Principal.WindowsPrincipal]::new(
  [Security.Principal.WindowsIdentity]::GetCurrent())
if (-not $principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) {
  Write-Error 'This script must run from an elevated PowerShell (scheduled task creation with highest run level requires admin).'
}

if ($Remove) {
  $existing = Get-ScheduledTask -TaskName $TASK_NAME -ErrorAction SilentlyContinue
  if ($existing) {
    Unregister-ScheduledTask -TaskName $TASK_NAME -Confirm:$false
    Write-Host "Removed scheduled task '$TASK_NAME'."
    Write-Host 'Re-enable autostart from the WinSpaces settings GUI if you still want the daemon at logon (non-elevated).'
  } else {
    Write-Host "Scheduled task '$TASK_NAME' does not exist; nothing to remove."
  }
  return
}

if (-not $ExePath) {
  # Default to the release build sitting next to this repo checkout.
  $root = Split-Path -Parent (Split-Path -Parent $MyInvocation.MyCommand.Path)
  $ExePath = Join-Path $root 'target\release\winspaces.exe'
}
if (-not (Test-Path $ExePath)) {
  Write-Error "Daemon executable not found: $ExePath (pass -ExePath or build with '.\dev build --release')"
}
$ExePath = (Resolve-Path $ExePath).Path

$action = New-ScheduledTaskAction -Execute $ExePath
$trigger = New-ScheduledTaskTrigger -AtLogOn -User $env:USERNAME
$taskPrincipal = New-ScheduledTaskPrincipal -UserId $env:USERNAME -LogonType Interactive -RunLevel Highest
$settings = New-ScheduledTaskSettingsSet -AllowStartIfOnBatteries -DontStopIfGoingOnBatteries `
  -DontStopOnIdleEnd -ExecutionTimeLimit ([TimeSpan]::Zero)

Register-ScheduledTask -TaskName $TASK_NAME -Action $action -Trigger $trigger `
  -Principal $taskPrincipal -Settings $settings -Force | Out-Null
Write-Host "Installed scheduled task '$TASK_NAME' -> $ExePath (runs elevated at logon, no UAC prompt)."

# Avoid a double start: the GUI's autostart toggle writes the HKCU Run key.
if (Get-ItemProperty -Path $RUN_KEY -Name $RUN_VALUE -ErrorAction SilentlyContinue) {
  Remove-ItemProperty -Path $RUN_KEY -Name $RUN_VALUE
  Write-Host "Removed the non-elevated HKCU Run autostart entry ('$RUN_VALUE') to prevent a double start."
}
