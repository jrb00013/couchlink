# Stream the Windows primary display to couchlink-host running in WSL (TCP CLFR on port 9876).
param(
    [string]$Bind = "0.0.0.0:9876",
    [int]$MaxFps = 60
)

$ErrorActionPreference = "Stop"
$Root = Split-Path -Parent (Split-Path -Parent $MyInvocation.MyCommand.Path)
$Bin = Join-Path $Root "target\release\couchlink-win-capture.exe"
if (-not (Test-Path $Bin)) {
    $Bin = Join-Path $Root "target\debug\couchlink-win-capture.exe"
}
if (-not (Test-Path $Bin)) {
    Write-Host "Building couchlink-win-capture..."
    Push-Location $Root
    cargo build -p couchlink-capture-bridge --bin couchlink-win-capture --release
    Pop-Location
    $Bin = Join-Path $Root "target\release\couchlink-win-capture.exe"
}

Write-Host "Windows capture bridge on $Bind (WSL: COUCHLINK_WINDOWS_CAPTURE=auto)"
Write-Host "If WSL cannot connect, allow inbound TCP 9876 in Windows Firewall."
& $Bin --bind $Bind --max-fps $MaxFps
