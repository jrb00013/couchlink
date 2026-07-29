# One command to run couchlink on native Windows: .\scripts\run.ps1 [client]
#
# Host role needs Linux uinput to inject the virtual Bluetooth DualSense, which
# native Windows doesn't have (roadmap: ViGEm path not yet implemented) — run
# the host from WSL or Linux instead: wsl ./scripts/run.sh host
#
# Native Windows can run the friend/client role: it opens a window showing the
# host's video and reads your local DualSense (or keyboard, if no DualSense is
# connected) and sends CLPD pad frames to a host running elsewhere.

param(
    [ValidateSet("client")]
    [string]$Role = "client"
)

$Root = Split-Path -Parent $PSScriptRoot
Set-Location $Root

Write-Host "==> platform: windows . role: $Role"
Write-Host "Host role requires Linux/WSL (uinput). Use: wsl ./scripts/run.sh host"

$EnvFile = Join-Path $Root ".env.couchlink"
if (-not (Test-Path $EnvFile)) {
    Copy-Item (Join-Path $Root ".env.example") $EnvFile
}

Get-Content $EnvFile | ForEach-Object {
    if ($_ -match "^\s*([A-Za-z_][A-Za-z0-9_]*)=(.*)$") {
        [System.Environment]::SetEnvironmentVariable($Matches[1], $Matches[2], "Process")
    }
}

if (-not $env:COUCHLINK_SESSION_ID -or -not $env:COUCHLINK_PIN) {
    if (-not $env:COUCHLINK_JOIN_URL) {
        Write-Host "No join URL in .env — Couchlink Player will prompt on startup."
    }
}

$exe = Join-Path $Root "target\release\couchlink-client.exe"
if (-not (Test-Path $exe)) {
    Write-Error "couchlink-client.exe not found — build it first: cargo build --release -p couchlink-client"
    exit 1
}

$argsList = @()
if ($env:COUCHLINK_JOIN_URL) {
    $argsList += @("--join-url", $env:COUCHLINK_JOIN_URL)
}
if ($env:COUCHLINK_SESSION_ID) { $argsList += @("--session-id", $env:COUCHLINK_SESSION_ID) }
if ($env:COUCHLINK_PIN) { $argsList += @("--pin", $env:COUCHLINK_PIN) }
if ($env:COUCHLINK_SIGNALING) { $argsList += @("--signaling", $env:COUCHLINK_SIGNALING) }
if ($env:COUCHLINK_TURN_URL) { $argsList += @("--turn-url", $env:COUCHLINK_TURN_URL) }
if ($env:COUCHLINK_TURN_USER) { $argsList += @("--turn-user", $env:COUCHLINK_TURN_USER) }
if ($env:COUCHLINK_TURN_PASS) { $argsList += @("--turn-pass", $env:COUCHLINK_TURN_PASS) }
if ($env:COUCHLINK_ICE_IPS) { $argsList += @("--ice-ips", $env:COUCHLINK_ICE_IPS) }

& $exe @argsList
