#Requires -Version 5.1
<#
.SYNOPSIS
  Install Couchlink Player for Windows (Start Menu + Desktop shortcut).

.EXAMPLE
  .\install-client.ps1
  .\install-client.ps1 -JoinUrl "http://203.0.113.10:8443/?s=night&p=123456&auto=1&ws=ws://203.0.113.10:8443/ws"
#>
param(
    [string]$JoinUrl = "",
    [string]$SourceExe = ""
)

$ErrorActionPreference = "Stop"
$InstallDir = Join-Path $env:LOCALAPPDATA "Couchlink"
New-Item -ItemType Directory -Force -Path $InstallDir | Out-Null

if (-not $SourceExe) {
    $Here = Split-Path -Parent $MyInvocation.MyCommand.Path
    $SourceExe = Join-Path $Here "couchlink-client.exe"
}
if (-not (Test-Path $SourceExe)) {
    Write-Error "couchlink-client.exe not found next to this script. Run build-release.ps1 first or pass -SourceExe."
}

Copy-Item -Force $SourceExe (Join-Path $InstallDir "couchlink-client.exe")

$ConfigPath = Join-Path $InstallDir "config"
if ($JoinUrl) {
    "join_url=$JoinUrl" | Set-Content -Encoding utf8 $ConfigPath
} elseif (-not (Test-Path $ConfigPath)) {
    @"
# Paste the host's join link on the next line (one line, no spaces):
join_url=
"@ | Set-Content -Encoding utf8 $ConfigPath
    Write-Host "Edit $ConfigPath and set join_url= to the link the host sent you."
}

$Wsh = New-Object -ComObject WScript.Shell
foreach ($LinkDir in @(
        [Environment]::GetFolderPath("Desktop"),
        (Join-Path $env:APPDATA "Microsoft\Windows\Start Menu\Programs")
    )) {
    $Lnk = Join-Path $LinkDir "Couchlink Player.lnk"
    $Sc = $Wsh.CreateShortcut($Lnk)
    $Sc.TargetPath = Join-Path $InstallDir "couchlink-client.exe"
    $Sc.WorkingDirectory = $InstallDir
    $Sc.Description = "Join a couchlink session"
    $Sc.Save()
}

Write-Host "Installed to $InstallDir"
Write-Host "Double-click 'Couchlink Player' on Desktop or Start Menu."
