#Requires -Version 5.1
<#
.SYNOPSIS
  Build CouchlinkHelper-Setup.exe (elevated service installer via Inno Setup 6).

.EXAMPLE
  .\packaging\windows\build-helper-installer.ps1
#>
$ErrorActionPreference = "Stop"
$Root = Split-Path -Parent (Split-Path -Parent $PSScriptRoot)
Set-Location $Root

Write-Host "==> building couchlink-helper (release)"
cargo build --release -p couchlink-windows-helper
if ($LASTEXITCODE -ne 0) { throw "cargo build failed" }

$Exe = Join-Path $Root "target\release\couchlink-helper.exe"
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

Binary is ready at:
  $Exe

Elevated one-shot without Inno:
  Start-Process -FilePath '$Exe' -Verb RunAs -ArgumentList 'install' -Wait
"@
    exit 1
}

New-Item -ItemType Directory -Force -Path (Join-Path $Root "build\windows") | Out-Null
& $Iscc (Join-Path $PSScriptRoot "couchlink-helper.iss")
if ($LASTEXITCODE -ne 0) { throw "ISCC failed" }

$Out = Join-Path $Root "build\windows\CouchlinkHelper-Setup-0.1.1.exe"
if (Test-Path $Out) {
    Write-Host "==> installer ready: $Out"
    Write-Host "    Double-click once (UAC) — then --online needs no elevation."
} else {
    throw "ISCC finished but installer not found at $Out"
}
