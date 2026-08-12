# Ensure couchlink-win-capture.exe exists (build on Windows if missing/stale).
param(
    [switch]$Force
)

$ErrorActionPreference = "Stop"
$Root = Split-Path -Parent (Split-Path -Parent $MyInvocation.MyCommand.Path)
$Bin = Join-Path $Root "target\release\couchlink-win-capture.exe"

$needBuild = $Force -or -not (Test-Path $Bin)
if (-not $needBuild) {
    $binTime = (Get-Item $Bin).LastWriteTimeUtc
    $srcDir = Join-Path $Root "crates\capture-bridge"
    $newestSrc = Get-ChildItem -Path $srcDir -Recurse -File -Include *.rs,Cargo.toml |
        Sort-Object LastWriteTimeUtc -Descending |
        Select-Object -First 1
    if ($newestSrc -and $newestSrc.LastWriteTimeUtc -gt $binTime) {
        $needBuild = $true
    }
}

if ($needBuild) {
    if (-not (Get-Command cargo -ErrorAction SilentlyContinue)) {
        throw "cargo not found on Windows - install Rust (https://rustup.rs) with the MSVC toolchain, then reopen your terminal"
    }
    Write-Host "==> building couchlink-win-capture.exe (Windows DXGI / Graphics Capture)"
    Push-Location $Root
    try {
        cargo build -p couchlink-capture-bridge --bin couchlink-win-capture --release
        if ($LASTEXITCODE -ne 0) {
            throw "cargo build failed with exit code $LASTEXITCODE"
        }
    } finally {
        Pop-Location
    }
}

if (-not (Test-Path $Bin)) {
    throw "couchlink-win-capture.exe missing after build: $Bin"
}

Write-Host "==> ready: $Bin"
$Bin
