#Requires -Version 5.1
# One-shot elevated fix: refresh Helper scripts in Program Files (no full reinstall).
param(
    [string]$SourceDir = ""
)
$ErrorActionPreference = "Stop"
$Dest = Join-Path ${env:ProgramFiles} "Couchlink\Helper"
if (-not $SourceDir) {
    $SourceDir = Split-Path -Parent $MyInvocation.MyCommand.Path
}
if (-not (Test-Path $Dest)) {
    throw "Helper not installed at $Dest — run install-windows-helper.sh first"
}
foreach ($name in @("enable-upnp.ps1", "unblock-firewall.ps1", "call-helper.ps1")) {
    $from = Join-Path $SourceDir $name
    if (-not (Test-Path $from)) { throw "missing $from" }
    Copy-Item -Force $from (Join-Path $Dest $name)
    Write-Host "OK updated $name"
}
$markerDir = Join-Path $env:ProgramData "Couchlink\run"
New-Item -ItemType Directory -Force -Path $markerDir | Out-Null
Set-Content -Path (Join-Path $markerDir "helper-scripts.exit") -Value "0" -Encoding ascii
Write-Host "OK Helper scripts refreshed — online_prep should work now"
