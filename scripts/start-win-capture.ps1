# Launch couchlink-win-capture (builds first if needed).
param(
    [string]$Connect = "127.0.0.1:9876",
    [ValidateSet("desktop", "picker", "window")]
    [string]$Source = "picker",
    [string]$Window = "",
    [int]$MaxFps = 60,
    [switch]$GpuEncode,
    [int]$MaxWidth = 1920,
    [int]$MaxHeight = 1080,
    [int]$BitrateKbps = 18000,
    [switch]$ListWindows,
    [switch]$BuildOnly
)

$ErrorActionPreference = "Stop"
$Root = Split-Path -Parent (Split-Path -Parent $MyInvocation.MyCommand.Path)

# docs/INCIDENT-2026-08-19-terminals-died.md: every console couchlink spawns
# (this one included) attaches into the user's interactive Windows Terminal
# by default, and enough of that from non-interactive tooling destabilizes
# it. Idempotent — a cheap registry read after the first run.
try {
    & (Join-Path $Root "scripts\windows\fix-default-terminal.ps1") | Out-Null
} catch {
    Write-Host "WARN: fix-default-terminal.ps1 failed (non-fatal): $($_.Exception.Message)"
}

$BuildScript = Join-Path $Root "scripts\build-win-capture.ps1"
$Bin = & $BuildScript
if (-not $Bin) { throw "build-win-capture.ps1 returned no binary path" }
$Bin = "$Bin".Trim()

# $Root resolves through the \\wsl.localhost\... UNC share this script was
# invoked from, so $Bin does too — and Windows shows a blocking "Open File -
# Security Warning" for an unsigned .exe run from a network location, with
# nobody there to click it since this runs from a background-spawned
# PowerShell. Every capture-picker-never-appeared symptom traced back to this:
# the exe never even started. Stage it to a real local NTFS path first so it's
# never in that zone to begin with — the fix, not a prompt-suppression hack.
$LocalDir = Join-Path $env:LOCALAPPDATA "couchlink\bin"
New-Item -ItemType Directory -Force -Path $LocalDir | Out-Null
$LocalBin = Join-Path $LocalDir "couchlink-win-capture.exe"
Copy-Item -Path $Bin -Destination $LocalBin -Force
$Bin = $LocalBin

if ($BuildOnly) { exit 0 }

if ($ListWindows) {
    & $Bin --list-windows
    exit $LASTEXITCODE
}

$argList = @("--connect", $Connect, "--max-fps", "$MaxFps", "--source", $Source,
             "--max-width", "$MaxWidth", "--max-height", "$MaxHeight",
             "--bitrate-kbps", "$BitrateKbps")
if ($GpuEncode) { $argList += @("--gpu-encode", "true") }
if ($Source -eq "window") {
    if (-not $Window) { throw "-Window is required when -Source window" }
    $argList += @("--window", $Window)
}

Write-Host "Windows capture: source=$Source connect=$Connect ${MaxWidth}x${MaxHeight} @ ${BitrateKbps}kbps"
& $Bin @argList
