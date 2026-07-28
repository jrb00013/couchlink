# Stream the Windows primary display to couchlink-host running in WSL (TCP CLFR on port 9876).
param(
    [string]$Bind = "0.0.0.0:9876",
    [int]$MaxFps = 60
)

$ErrorActionPreference = "Stop"
$Root = Split-Path -Parent (Split-Path -Parent $MyInvocation.MyCommand.Path)

# Already serving? Reuse the existing process.
$port = 9876
if ($Bind -match ':(\d+)$') { $port = [int]$Matches[1] }
$existing = Get-NetTCPConnection -LocalPort $port -State Listen -ErrorAction SilentlyContinue
if ($existing) {
    Write-Host "Windows capture already listening on port $port — nothing to do."
    exit 0
}

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

Write-Host "Windows capture bridge on $Bind (WSL host connects automatically)"
Write-Host "If WSL cannot connect, allow inbound TCP $port in Windows Firewall."
& $Bin --bind $Bind --max-fps $MaxFps
