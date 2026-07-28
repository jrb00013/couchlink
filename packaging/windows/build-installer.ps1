#Requires -Version 5.1
<#
.SYNOPSIS
  Build CouchlinkPlayer-Setup.exe (real Windows installer via Inno Setup 6).

.EXAMPLE
  .\packaging\windows\build-installer.ps1
#>
$ErrorActionPreference = "Stop"
$Root = Split-Path -Parent (Split-Path -Parent $PSScriptRoot)
Set-Location $Root

Write-Host "==> building couchlink-client (release)"
cargo build --release -p couchlink-client

$Exe = Join-Path $Root "target\release\couchlink-client.exe"
if (-not (Test-Path $Exe)) { throw "missing $Exe" }

$IsccCandidates = @(
    "${env:ProgramFiles(x86)}\Inno Setup 6\ISCC.exe",
    "${env:ProgramFiles}\Inno Setup 6\ISCC.exe"
)
$Iscc = $IsccCandidates | Where-Object { Test-Path $_ } | Select-Object -First 1
if (-not $Iscc) {
    Write-Host @"

Inno Setup 6 is required to build the installer:
  https://jrsoftware.org/isdl.php

After installing, re-run this script.
"@
    exit 1
}

New-Item -ItemType Directory -Force -Path (Join-Path $Root "build\windows") | Out-Null
& $Iscc (Join-Path $PSScriptRoot "couchlink-player.iss")
$Out = Join-Path $Root "build\windows\CouchlinkPlayer-Setup-0.1.1.exe"
if (Test-Path $Out) {
    Write-Host "==> installer ready: $Out"
    Write-Host "    Send this single .exe to your friend — double-click to install."
} else {
    throw "ISCC finished but installer not found at $Out"
}
