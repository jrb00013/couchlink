#Requires -Version 5.1
<#
.SYNOPSIS
  Install Tailscale for Windows (used from WSL setup-tailscale.sh --ensure).
  Prefer official MSI; winget is optional and non-blocking.
#>
param(
    [switch]$SkipLaunch
)

$ErrorActionPreference = "Continue"
$LogDir = Join-Path $env:LOCALAPPDATA "couchlink-run"
New-Item -ItemType Directory -Force -Path $LogDir | Out-Null
$Marker = Join-Path $LogDir "install-tailscale.exit"
$Log = Join-Path $LogDir "install-tailscale.log"

function Write-Log([string]$Msg) {
    $line = "[{0}] {1}" -f (Get-Date -Format "u"), $Msg
    Add-Content -Path $Log -Value $line
    Write-Host $Msg
}

function Find-TailscaleExe {
    $cands = @(
        "${env:ProgramFiles}\Tailscale\tailscale.exe",
        "${env:ProgramFiles(x86)}\Tailscale\tailscale.exe",
        "$env:LOCALAPPDATA\Tailscale\tailscale.exe"
    )
    foreach ($p in $cands) {
        if (Test-Path -LiteralPath $p) { return $p }
    }
    $cmd = Get-Command tailscale.exe -ErrorAction SilentlyContinue
    if ($cmd) { return $cmd.Source }
    return $null
}

function Finish([int]$Code) {
    Set-Content -Path $Marker -Value "$Code" -Encoding ascii
    exit $Code
}

Write-Log "Couchlink: install Tailscale for Windows"

$existing = Find-TailscaleExe
if ($existing) {
    Write-Log "already installed: $existing"
    if (-not $SkipLaunch) {
        Start-Process "tailscale://" -ErrorAction SilentlyContinue
    }
    Finish 0
}

[Net.ServicePointManager]::SecurityProtocol = [Net.SecurityProtocolType]::Tls12

# 1) Official MSI (amd64) - most reliable from WSL / automation
$msiUrl = "https://pkgs.tailscale.com/stable/tailscale-setup-latest-amd64.msi"
$msi = Join-Path $env:TEMP "couchlink-tailscale-setup.msi"
Write-Log "Downloading $msiUrl"
try {
    Invoke-WebRequest -Uri $msiUrl -OutFile $msi -UseBasicParsing
} catch {
    Write-Log ("MSI download failed: " + $_.Exception.Message)
    $msi = $null
}

if ($msi -and (Test-Path -LiteralPath $msi)) {
    Write-Log "msiexec /qn install (approve Windows UAC when prompted)"
    $msiArgs = "/i `"$msi`" /qn /norestart"
    $attempts = 0
    while ($attempts -lt 3) {
        $attempts++
        try {
            $p = Start-Process -FilePath "msiexec.exe" -ArgumentList $msiArgs -Verb RunAs -Wait -PassThru
            Write-Log ("msiexec elevated exit=" + $p.ExitCode + " (attempt $attempts)")
            if ($p.ExitCode -eq 1618) {
                Write-Log "installer busy (1618) - waiting 15s"
                Start-Sleep -Seconds 15
                continue
            }
            break
        } catch {
            Write-Log ("elevated msiexec failed: " + $_.Exception.Message)
            break
        }
    }
    Start-Sleep -Seconds 2
    $existing = Find-TailscaleExe
    if ($existing) {
        Write-Log "installed via MSI: $existing"
        if (-not $SkipLaunch) {
            Start-Process "tailscale://" -ErrorAction SilentlyContinue
            Start-Process -FilePath $existing -ArgumentList @("up") -ErrorAction SilentlyContinue
        }
        Finish 0
    }
}

# 2) winget fallback (disable interactivity; short timeout via job)
$winget = Get-Command winget.exe -ErrorAction SilentlyContinue
if ($null -ne $winget) {
    Write-Log "winget fallback install Tailscale.Tailscale"
    $wgArgs = @(
        "install", "--id", "Tailscale.Tailscale", "-e",
        "--silent", "--disable-interactivity",
        "--accept-package-agreements", "--accept-source-agreements",
        "--architecture", "x64"
    )
    $job = Start-Job -ScriptBlock {
        param($exe, $a)
        & $exe @a
    } -ArgumentList $winget.Source, $wgArgs
    if (Wait-Job $job -Timeout 180) {
        Receive-Job $job | ForEach-Object { Write-Log "$_" }
    } else {
        Write-Log "winget timed out after 180s - stopping"
        Stop-Job $job -ErrorAction SilentlyContinue
    }
    Remove-Job $job -Force -ErrorAction SilentlyContinue
    Start-Sleep -Seconds 2
    $existing = Find-TailscaleExe
    if ($existing) {
        Write-Log "installed via winget: $existing"
        if (-not $SkipLaunch) {
            Start-Process "tailscale://" -ErrorAction SilentlyContinue
        }
        Finish 0
    }
}

# 3) No more reliable silent EXE URL on pkgs.tailscale.com (404s).
Write-Log "Tailscale still not found - approve the Windows UAC prompt and re-run:"
Write-Log "  ./scripts/setup-tailscale.sh --ensure"
Finish 1
