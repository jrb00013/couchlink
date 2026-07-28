; Couchlink Player — Windows installer (Inno Setup 6)
; Build: packaging\windows\build-installer.ps1  (or GitHub Actions release-player workflow)

#define MyAppName "Couchlink Player"
#define MyAppVersion "0.1.1"
#define MyAppPublisher "couchlink"
#define MyAppExeName "couchlink-client.exe"
#define MyAppURL "https://github.com/jrb00013/couchlink"

[Setup]
AppId={{8F4E2A1B-3C5D-4E6F-9A0B-1C2D3E4F5A6B}
AppName={#MyAppName}
AppVersion={#MyAppVersion}
AppPublisher={#MyAppPublisher}
AppPublisherURL={#MyAppURL}
AppSupportURL={#MyAppURL}
AppUpdatesURL={#MyAppURL}
DefaultDirName={localappdata}\Couchlink
DefaultGroupName={#MyAppName}
DisableProgramGroupPage=yes
PrivilegesRequired=lowest
OutputDir=..\..\build\windows
OutputBaseFilename=CouchlinkPlayer-Setup-{#MyAppVersion}
Compression=lzma2
SolidCompression=yes
WizardStyle=modern
UninstallDisplayIcon={app}\{#MyAppExeName}
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible

[Languages]
Name: "english"; MessagesFile: "compiler:Default.isl"

[Tasks]
Name: "desktopicon"; Description: "{cm:CreateDesktopIcon}"; GroupDescription: "{cm:AdditionalIcons}"; Flags: unchecked

[Files]
Source: "..\..\target\release\{#MyAppExeName}"; DestDir: "{app}"; Flags: ignoreversion

[Icons]
Name: "{group}\{#MyAppName}"; Filename: "{app}\{#MyAppExeName}"; WorkingDir: "{app}"
Name: "{autodesktop}\{#MyAppName}"; Filename: "{app}\{#MyAppExeName}"; WorkingDir: "{app}"; Tasks: desktopicon

[Run]
Filename: "{app}\{#MyAppExeName}"; Description: "Launch {#MyAppName}"; Flags: nowait postinstall skipifsilent

[Code]
var
  JoinUrlPage: TInputQueryWizardPage;

procedure InitializeWizard;
begin
  JoinUrlPage := CreateInputQueryPage(wpSelectDir,
    'Invite link', 'Paste the join link from your friend (host)',
    'You can leave this blank and edit the config file later. The host sends a long URL with ?s= and ?p= in it.');
  JoinUrlPage.Add('Join URL (optional):', False);
end;

procedure CurStepChanged(CurStep: TSetupStep);
var
  ConfigPath, Url, Content: String;
begin
  if CurStep = ssPostInstall then
  begin
    ConfigPath := ExpandConstant('{app}\config');
    Url := Trim(JoinUrlPage.Values[0]);
    if Url <> '' then
      Content := 'join_url=' + Url
    else
      Content := '# Paste the host''s join link on the next line:' + #13#10 + 'join_url=' + #13#10;
    SaveStringToFile(ConfigPath, Content, False);
  end;
end;

[UninstallDelete]
Type: files; Name: "{app}\config"
