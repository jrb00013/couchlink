#Requires -Version 5.1
<#
.SYNOPSIS
  Build release couchlink-client.exe for packaging.

.EXAMPLE
  .\packaging\windows\build-release.ps1
#>
$ErrorActionPreference = "Stop"
$Root = Split-Path -Parent (Split-Path -Parent $PSScriptRoot)
Set-Location $Root

Write-Host "==> building couchlink-client (release)"
cargo build --release -p couchlink-client

$Exe = Join-Path $Root "target\release\couchlink-client.exe"
if (-not (Test-Path $Exe)) { throw "build failed: $Exe missing" }

$OutDir = Join-Path $Root "build\windows"
New-Item -ItemType Directory -Force -Path $OutDir | Out-Null
$Zip = Join-Path $OutDir "CouchlinkPlayer-win64.zip"
if (Test-Path $Zip) { Remove-Item $Zip }

$Stage = Join-Path $env:TEMP "couchlink-player-stage"
if (Test-Path $Stage) { Remove-Item -Recurse -Force $Stage }
New-Item -ItemType Directory -Force -Path $Stage | Out-Null
Copy-Item $Exe $Stage
Copy-Item (Join-Path $Root "packaging\config.example") (Join-Path $Stage "config.example")
Copy-Item (Join-Path $PSScriptRoot "install-client.ps1") $Stage

Compress-Archive -Path (Join-Path $Stage "*") -DestinationPath $Zip -Force
Write-Host "==> wrote $Zip"
Write-Host "Friend: unzip, run install-client.ps1, paste join URL when prompted"
