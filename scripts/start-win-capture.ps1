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
$BuildScript = Join-Path $Root "scripts\build-win-capture.ps1"
$Bin = & $BuildScript
if (-not $Bin) { throw "build-win-capture.ps1 returned no binary path" }
$Bin = "$Bin".Trim()

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
