#Requires -Version 5.1
<#
.SYNOPSIS
  Allow Tailscale / Headscale / couchlink ports through Windows Firewall.
#>
$ErrorActionPreference = "Continue"
$LogDir = Join-Path $env:LOCALAPPDATA "couchlink-run"
New-Item -ItemType Directory -Force -Path $LogDir | Out-Null
$Marker = Join-Path $LogDir "unblock-firewall.exit"

function Ok([string]$m) { Write-Host "OK: $m" }
function Warn([string]$m) { Write-Host "WARN: $m" }

function Ensure-Rule([string]$Name, [string]$Protocol, [int]$Port) {
    $existing = Get-NetFirewallRule -DisplayName $Name -ErrorAction SilentlyContinue
    if ($existing) {
        Ok "rule exists: $Name"
        return
    }
    try {
        New-NetFirewallRule -DisplayName $Name -Direction Inbound -Action Allow `
            -Protocol $Protocol -LocalPort $Port -Profile Any | Out-Null
        Ok "added $Name ($Protocol/$Port)"
    } catch {
        Warn ("failed $Name : " + $_.Exception.Message)
    }
}

Write-Host "Couchlink: unblock Windows firewall"

Ensure-Rule "couchlink-signaling-8443" "TCP" 8443
Ensure-Rule "couchlink-turn-3478-tcp" "TCP" 3478
Ensure-Rule "couchlink-turn-3478-udp" "UDP" 3478
Ensure-Rule "couchlink-headscale-stun-3479" "UDP" 3479
Ensure-Rule "tailscale-wireguard-41641" "UDP" 41641

# Allow Tailscale binaries if present
$bins = @(
    "${env:ProgramFiles}\Tailscale\tailscale.exe",
    "${env:ProgramFiles}\Tailscale\tailscaled.exe"
)
foreach ($b in $bins) {
    if (Test-Path $b) {
        $n = "couchlink-allow-" + [IO.Path]::GetFileNameWithoutExtension($b)
        try {
            New-NetFirewallRule -DisplayName $n -Direction Inbound -Action Allow `
                -Program $b -Profile Any -ErrorAction SilentlyContinue | Out-Null
            Ok "program allow: $b"
        } catch {}
    }
}

Set-Content -Path $Marker -Value "0" -Encoding ascii
exit 0
