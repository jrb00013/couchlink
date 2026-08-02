# Run DualSense VHID companion (Player 2 for RPCS3/PCSX2)
# Requires ViGEmBus: https://github.com/nefarius/ViGEmBus/releases
param(
    [ValidateSet("ds4", "xbox360")]
    [string]$Backend = "ds4"
)

$ErrorActionPreference = "Stop"
$RepoRoot = Resolve-Path (Join-Path $PSScriptRoot "..\..")
Set-Location $RepoRoot

cargo build -p couchlink-ds-vhid --release
$Exe = Join-Path $RepoRoot "target\release\couchlink-ds-vhid.exe"
if (-not (Test-Path $Exe)) {
    throw "Build did not produce $Exe (run on native Windows with MSVC toolchain)"
}

& $Exe --backend $Backend
