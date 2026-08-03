# Ensure couchlink-ds-vhid.exe exists (build on Windows if missing/stale)
# and that a virtual-pad driver is present.
param(
    [switch]$Force
)

$ErrorActionPreference = "Stop"
$Root = Split-Path -Parent (Split-Path -Parent $MyInvocation.MyCommand.Path)
$Bin = Join-Path $Root "target\release\couchlink-ds-vhid.exe"

# ViGEmBus is what turns the companion into a controller Windows games can see.
# Without it the companion starts but every backend fails at runtime.
function Test-ViGEmBus {
    $svc = Get-Service -Name "ViGEmBus" -ErrorAction SilentlyContinue
    if ($svc) { return $true }
    $drv = Get-ChildItem "$env:SystemRoot\System32\drivers\ViGEmBus.sys" -ErrorAction SilentlyContinue
    return [bool]$drv
}

if (-not (Test-ViGEmBus)) {
    Write-Host "==> ViGEmBus not found — installing (virtual gamepad driver)"
    $winget = Get-Command winget -ErrorAction SilentlyContinue
    if ($winget) {
        # Driver install triggers its own UAC prompt; approve it.
        winget install --id ViGEm.ViGEmBus --silent --accept-package-agreements --accept-source-agreements
        if ($LASTEXITCODE -ne 0) {
            Write-Warning "winget could not install ViGEmBus (exit $LASTEXITCODE)"
        }
    } else {
        Write-Warning "winget unavailable — install ViGEmBus manually:"
        Write-Warning "  https://github.com/nefarius/ViGEmBus/releases"
    }
    if (-not (Test-ViGEmBus)) {
        Write-Warning "ViGEmBus still missing — controller input will not reach games."
        Write-Warning "Video streaming still works; install the driver and re-run."
    } else {
        Write-Host "OK  ViGEmBus installed"
    }
}

$needBuild = $Force -or -not (Test-Path $Bin)
if (-not $needBuild) {
    $binTime = (Get-Item $Bin).LastWriteTimeUtc
    $newestSrc = Get-ChildItem -Path (Join-Path $Root "crates\ds-vhid"), (Join-Path $Root "crates\pad") `
        -Recurse -File -Include *.rs,Cargo.toml -ErrorAction SilentlyContinue |
        Sort-Object LastWriteTimeUtc -Descending |
        Select-Object -First 1
    if ($newestSrc -and $newestSrc.LastWriteTimeUtc -gt $binTime) {
        $needBuild = $true
    }
}

if ($needBuild) {
    Write-Host "==> building couchlink-ds-vhid.exe (virtual DualSense companion)"
    if (-not (Get-Command cargo -ErrorAction SilentlyContinue)) {
        throw "cargo not found on Windows — install Rust (https://rustup.rs) with the MSVC toolchain"
    }
    Push-Location $Root
    try {
        cargo build -p couchlink-ds-vhid --release
        if ($LASTEXITCODE -ne 0) {
            throw "cargo build failed with exit code $LASTEXITCODE"
        }
    } finally {
        Pop-Location
    }
}

if (-not (Test-Path $Bin)) {
    throw "couchlink-ds-vhid.exe missing after build: $Bin"
}

Write-Host "==> ready: $Bin"
$Bin
