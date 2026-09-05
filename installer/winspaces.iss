; WinSpaces installer (Inno Setup 6).
; Compiled by scripts\make-installer.ps1, which passes:
;   /DAppVersion=<x.y.z>   version parsed from crates\winspaces\Cargo.toml
;   /DStageDir=<path>      staged payload (daemon exe + recovery scripts)
;
; Design decisions (see docs\distribution.md):
;   - Per-user install, no UAC (PrivilegesRequired=lowest) — matches the
;     non-elevated daemon posture (docs\ipc-and-config.md §6).
;   - Autostart task writes the same HKCU Run value the GUI toggle manages.
;   - The opt-in elevated scheduled task is NOT created here; power users run
;     {app}\scripts\install-elevated-autostart.ps1 from an elevated shell.

#ifndef AppVersion
  #define AppVersion "0.0.0"
#endif
#ifndef StageDir
  #define StageDir "..\dist\staging"
#endif

#define AppName "WinSpaces"
#define AppPublisher "izantech"
#define AppURL "https://github.com/izantech/winspaces"
#define DaemonExe "winspaces.exe"

[Setup]
AppId={{7F1FA3E1-4D2B-4E1C-9B7A-2C54A0E63D11}
AppName={#AppName}
AppVersion={#AppVersion}
AppPublisher={#AppPublisher}
AppPublisherURL={#AppURL}
AppSupportURL={#AppURL}/issues
AppUpdatesURL={#AppURL}/releases
; `lowest` pins Setup to non-administrative install mode even when a user
; launches it elevated, so {autopf} always resolves to the per-user
; %LOCALAPPDATA%\Programs and stays consistent with the HKCU Run value below.
DefaultDirName={autopf}\{#AppName}
DefaultGroupName={#AppName}
DisableProgramGroupPage=yes
PrivilegesRequired=lowest
LicenseFile=..\LICENSE
OutputBaseFilename=WinSpaces-Setup-x64-{#AppVersion}
SetupIconFile=
Compression=lzma2/max
SolidCompression=yes
WizardStyle=modern
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
UninstallDisplayIcon={app}\{#DaemonExe}
; Deliberately off: Restart Manager would hard-close a running daemon and
; strand every cloaked window. PrepareToInstall below stops it gracefully
; with --exit instead, which uncloaks everything before files are replaced.
CloseApplications=no
; Stamps the *setup* executable only; winspaces.exe carries no VERSIONINFO.
VersionInfoVersion={#AppVersion}

[Languages]
Name: "english"; MessagesFile: "compiler:Default.isl"
Name: "spanish"; MessagesFile: "compiler:Languages\Spanish.isl"

; Every user-visible literal of this script, per language. The scheduled-task
; name in [UninstallRun] is an identifier and stays as it is.
[CustomMessages]
english.AutostartTask=Start {#AppName} automatically at login
english.SettingsShortcut={#AppName} Settings
english.MissionControlShortcut=Mission Control
english.LaunchNow=Launch {#AppName} now
spanish.AutostartTask=Iniciar {#AppName} automáticamente al iniciar sesión
spanish.SettingsShortcut=Ajustes de {#AppName}
spanish.MissionControlShortcut=Mission Control
spanish.LaunchNow=Abrir {#AppName} ahora

[Tasks]
Name: "autostart"; Description: "{cm:AutostartTask}"
Name: "desktopicon"; Description: "{cm:CreateDesktopIcon}"; Flags: unchecked

[Files]
Source: "{#StageDir}\*"; DestDir: "{app}"; Flags: ignoreversion recursesubdirs createallsubdirs

[Icons]
Name: "{group}\{cm:SettingsShortcut}"; Filename: "{app}\{#DaemonExe}"; Parameters: "--settings"
Name: "{group}\{cm:MissionControlShortcut}"; Filename: "{app}\{#DaemonExe}"; Parameters: "--mission-control"
Name: "{group}\{#AppName}"; Filename: "{app}\{#DaemonExe}"
Name: "{autodesktop}\{cm:SettingsShortcut}"; Filename: "{app}\{#DaemonExe}"; Parameters: "--settings"; Tasks: desktopicon

[Registry]
; Same key/value the settings window's autostart toggle manages, so both stay in sync.
Root: HKCU; Subkey: "Software\Microsoft\Windows\CurrentVersion\Run"; \
  ValueType: string; ValueName: "{#AppName}"; ValueData: """{app}\{#DaemonExe}"""; \
  Flags: uninsdeletevalue; Tasks: autostart

[Run]
Filename: "{app}\{#DaemonExe}"; Description: "{cm:LaunchNow}"; \
  Flags: nowait postinstall skipifsilent

[UninstallRun]
; Graceful daemon shutdown restores every cloaked window before files vanish.
Filename: "{app}\{#DaemonExe}"; Parameters: "--exit"; \
  RunOnceId: "StopDaemon"; Flags: skipifdoesntexist
; Remove the opt-in elevated scheduled task if the user ever installed it.
Filename: "{sys}\schtasks.exe"; \
  Parameters: "/Delete /TN ""WinSpaces Daemon (Elevated)"" /F"; \
  RunOnceId: "RemoveElevatedTask"; Flags: runhidden skipifdoesntexist

[Code]
// Stop a running daemon gracefully before copying files over it. --exit
// uncloaks all managed windows, so an upgrade never strands hidden windows.
procedure StopRunningDaemon;
var
  OldExe: string;
  ResultCode: integer;
  Tries: integer;
  F: TFileStream;
begin
  OldExe := ExpandConstant('{app}\{#DaemonExe}');
  if not FileExists(OldExe) then
    exit;
  Exec(OldExe, '--exit', '', SW_HIDE, ewWaitUntilTerminated, ResultCode);
  // The control process returns immediately; the daemon needs a moment to
  // uncloak windows and exit. A running exe keeps its image file locked, so
  // poll until an exclusive open succeeds (fmOpenReadWrite or fmShareExclusive).
  for Tries := 0 to 20 do
  begin
    try
      F := TFileStream.Create(OldExe, $0002 or $0010);
      F.Free;
      break;
    except
      Sleep(250);
    end;
  end;
end;

function PrepareToInstall(var NeedsRestart: Boolean): String;
begin
  StopRunningDaemon;
  Result := '';
end;

procedure CurUninstallStepChanged(CurUninstallStep: TUninstallStep);
begin
  if CurUninstallStep = usUninstall then
    // Give --exit (fired by UninstallRun) a moment to restore windows.
    Sleep(1500);
end;
