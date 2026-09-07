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

# Only one launcher may own the capture exe. Host respawn + ensure used to
# stack two powershell retry loops → two win-capture.exe → Hyper-V starve.
$mutex = $null
try {
    $mutex = New-Object System.Threading.Mutex($false, "Global\CouchlinkWinCaptureLauncher")
    if (-not $mutex.WaitOne(0)) {
        Write-Host "Windows capture: another launcher already owns the mutex - exiting"
        exit 0
    }
} catch {
    Write-Host "WARN: could not take capture mutex: $($_.Exception.Message)"
}

# docs/INCIDENT-2026-08-19-terminals-died.md: every console couchlink spawns
# (this one included) attaches into the user's interactive Windows Terminal
# by default, and enough of that from non-interactive tooling destabilizes
# it. Idempotent - a cheap registry read after the first run.
try {
    & (Join-Path $Root "scripts\windows\fix-default-terminal.ps1") | Out-Null
} catch {
    Write-Host "WARN: fix-default-terminal.ps1 failed (non-fatal): $($_.Exception.Message)"
}

# A build failure must not strand a session that already has a working exe.
# This script is on the host's respawn path, so throwing here means capture
# never comes back for as long as the build keeps failing — even though the
# staged copy below is the very binary a successful build would have used.
# See the matching fallback in ensure-win-capture.sh.
$StagedBin = Join-Path $env:LOCALAPPDATA "couchlink\bin\couchlink-win-capture.exe"
$Bin = ""
try {
    $built = @(& (Join-Path $Root "scripts\build-win-capture.ps1"))
    $Bin = "$($built | Select-Object -Last 1)".Trim()
} catch {
    Write-Host "WARN: build-win-capture.ps1 failed: $($_.Exception.Message)"
}
if (-not $Bin) {
    if (Test-Path $StagedBin) {
        Write-Host "WARN: build produced no binary - falling back to staged $StagedBin"
        $Bin = $StagedBin
    } else {
        throw "build-win-capture.ps1 returned no binary path and none is staged at $StagedBin"
    }
}

# $Root resolves through the \\wsl.localhost\... UNC share this script was
# invoked from, so $Bin does too - and Windows shows a blocking "Open File -
# Security Warning" for an unsigned .exe run from a network location, with
# nobody there to click it since this runs from a background-spawned
# PowerShell. Every capture-picker-never-appeared symptom traced back to this:
# the exe never even started. Stage it to a real local NTFS path first so it's
# never in that zone to begin with - the fix, not a prompt-suppression hack.
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
try {
    if ($Source -eq "window") {
        # Title match can race the emulator: host start must not require PCSX2 to
        # already exist. Retry until the window appears (or capture ends cleanly).
        while ($true) {
            & $Bin @argList
            $code = $LASTEXITCODE
            if ($null -eq $code -or $code -eq 0) { break }
            # 75 = the captured window was destroyed (emulator restarted or the
            # game was closed). Relaunching re-resolves the title against the
            # new window, which is the whole point - see on_closed in
            # win_capture.rs. Any other non-zero code is the original case:
            # the window does not exist yet.
            if ($code -eq 75) {
                Write-Host "Windows capture: '$Window' window closed - reattaching in 2s"
            } else {
                Write-Host "Windows capture: no window matching '$Window' yet (exit $code) - retrying in 2s"
            }
            Start-Sleep -Seconds 2
        }
    } else {
        & $Bin @argList
    }
} finally {
    if ($null -ne $mutex) {
        try { $mutex.ReleaseMutex() } catch {}
        $mutex.Dispose()
    }
}
