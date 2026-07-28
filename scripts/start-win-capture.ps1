# Stream the Windows primary display to couchlink-host in WSL.
# Connects outbound to the WSL listener (default 127.0.0.1:9876 via localhost forwarding).
param(
    [string]$Connect = "127.0.0.1:9876",
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
    try {
        cargo build -p couchlink-capture-bridge --bin couchlink-win-capture --release
    } finally {
        Pop-Location
    }
    $Bin = Join-Path $Root "target\release\couchlink-win-capture.exe"
}
if (-not (Test-Path $Bin)) {
    throw "couchlink-win-capture.exe not found after build at $Bin"
}

Write-Host "Windows capture connecting to $Connect (WSL must listen)"
& $Bin --connect $Connect --max-fps $MaxFps
