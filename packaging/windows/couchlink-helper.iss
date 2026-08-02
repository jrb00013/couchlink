; Couchlink Helper — elevated Windows installer (Inno Setup 6)
; Build: packaging\windows\build-helper-installer.ps1
; Installs LocalSystem service so --online never needs UAC after setup.

#define MyAppName "Couchlink Helper"
#define MyAppVersion "0.1.1"
#define MyAppPublisher "couchlink"
#define MyAppURL "https://github.com/jrb00013/couchlink"
#define MyAppExeName "couchlink-helper.exe"

[Setup]
AppId={{A1B2C3D4-E5F6-7890-ABCD-EF1234567890}
AppName={#MyAppName}
AppVersion={#MyAppVersion}
AppPublisher={#MyAppPublisher}
AppPublisherURL={#MyAppURL}
AppSupportURL={#MyAppURL}
AppUpdatesURL={#MyAppURL}
DefaultDirName={commonpf}\Couchlink\Helper
DefaultGroupName={#MyAppName}
DisableProgramGroupPage=yes
PrivilegesRequired=admin
OutputDir=..\..\build\windows
OutputBaseFilename=CouchlinkHelper-Setup-{#MyAppVersion}
Compression=lzma2
SolidCompression=yes
WizardStyle=modern
UninstallDisplayIcon={app}\{#MyAppExeName}
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
CloseApplications=force

[Languages]
Name: "english"; MessagesFile: "compiler:Default.isl"

[Files]
Source: "..\..\target\release\{#MyAppExeName}"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\..\scripts\windows\enable-upnp.ps1"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\..\scripts\windows\unblock-firewall.ps1"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\..\scripts\windows\call-helper.ps1"; DestDir: "{app}"; Flags: ignoreversion

[Icons]
Name: "{group}\{#MyAppName} (docs)"; Filename: "{#MyAppURL}"

[Run]
Filename: "{app}\{#MyAppExeName}"; Parameters: "install"; StatusMsg: "Installing Couchlink Helper service…"; Flags: runhidden waituntilterminated

[UninstallRun]
Filename: "{app}\{#MyAppExeName}"; Parameters: "uninstall"; RunOnceId: "UninstallHelperService"; Flags: runhidden waituntilterminated

[Code]
function InitializeSetup(): Boolean;
begin
  Result := True;
  MsgBox('Couchlink Helper installs a Windows service that opens firewall rules and WSL portproxy without UAC prompts on every run.' + #13#10 + #13#10 +
    'You will approve Windows UAC once for this installer. After that, ./scripts/run.sh host --online needs no elevation.',
    mbInformation, MB_OK);
end;
