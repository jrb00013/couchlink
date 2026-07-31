# Build and run the native couchlink client on Windows.
#
# The point of running it here rather than in WSL: under WSL the viewer gets no
# real GPU (Mesa falls back to llvmpipe, then the surface fails), and it would
# still be paying for WSLg on top. On Windows it talks to the GPU directly and
# skips both WSLg and the browser compositor — the two costs the browser path
# cannot avoid.
param(
    [Parameter(Mandatory = $true)]
    [string]$JoinUrl,
    [switch]$Headless,
    [switch]$BuildOnly
)

$ErrorActionPreference = "Stop"
$Root = Split-Path -Parent (Split-Path -Parent $MyInvocation.MyCommand.Path)
Push-Location $Root
try {
    # Incremental compilation fails over the \\wsl.localhost share (the lock file
    # cannot be created), so it is disabled rather than left to error out.
    $env:CARGO_INCREMENTAL = "0"
    Write-Host "==> building couchlink-client (release, Windows)"
    cargo build --release -p couchlink-client
    if ($LASTEXITCODE -ne 0) { throw "cargo build failed" }

    $Bin = Join-Path $Root "target\release\couchlink-client.exe"
    if (-not (Test-Path $Bin)) { throw "binary not found at $Bin" }
    Write-Host "==> ready: $Bin"
    if ($BuildOnly) { exit 0 }

    $argList = @("--join-url", $JoinUrl)
    if ($Headless) { $argList += "--headless" }
    & $Bin @argList
}
finally {
    Pop-Location
}
