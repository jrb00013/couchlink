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

# Tailscale paste-link (http://100.x… / mesh=tailscale): ensure the friend can route.
$NeedsTs = $false
if ($JoinUrl -match 'mesh=tailscale' -or $JoinUrl -match '://100\.(6[4-9]|[7-9][0-9]|1[01][0-9]|12[0-7])\.') {
    $NeedsTs = $true
}
if ($NeedsTs) {
    Write-Host ""
    Write-Host "This join link uses Tailscale (same tailnet as the host)."
    $tsCmd = Get-Command tailscale -ErrorAction SilentlyContinue
    if (-not $tsCmd) {
        $cand = @(
            "$env:ProgramFiles\Tailscale\tailscale.exe",
            "${env:ProgramFiles(x86)}\Tailscale\tailscale.exe",
            "$env:LOCALAPPDATA\Tailscale\tailscale.exe"
        ) | Where-Object { Test-Path $_ } | Select-Object -First 1
        if ($cand) { $tsCmd = Get-Item $cand }
    }
    if (-not $tsCmd) {
        Write-Host "Tailscale not found — opening download page. Install, sign in (same account / shared node), then open Couchlink Player."
        Start-Process "https://tailscale.com/download/windows"
    } else {
        $ip = & $tsCmd.Source ip -4 2>$null
        if (-not $ip) {
            Write-Host "Tailscale installed but not signed in — open the Tailscale app, then Couchlink Player."
            Start-Process "tailscale://" -ErrorAction SilentlyContinue
        } else {
            Write-Host "Tailscale ready ($ip). Open Couchlink Player and paste/confirm the join link."
        }
    }
}
