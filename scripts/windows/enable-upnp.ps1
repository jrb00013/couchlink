#Requires -RunAsAdministrator
# Prepare Windows so real router UPnP can work, then try mapping Couchlink ports.
# Registers CouchlinkElevatedUpnp so later --online runs need no UAC (schtasks /Run).
#
# Exit: 0 = IGD OK / maps applied (or -SkipMap prep done)
#       2 = Windows prepared but router IGD still missing
#       1 = hard failure
param(
    [switch]$SkipMap,
    [string]$LanIp = "",
    [string]$RunDir = ""
)

$ErrorActionPreference = "Continue"
$RunDir = if ($RunDir) { $RunDir } else { Join-Path $env:LOCALAPPDATA "couchlink-run" }
New-Item -ItemType Directory -Force -Path $RunDir | Out-Null
$Marker = Join-Path $RunDir "enable-upnp.exit"
$LogFile = Join-Path $RunDir "enable-upnp.log"
$ScriptSelf = if ($PSCommandPath) { $PSCommandPath } else { $MyInvocation.MyCommand.Path }

function Log([string]$m, [string]$color = "White") {
    Write-Host $m -ForegroundColor $color
    Add-Content -Path $LogFile -Value $m -Encoding utf8 -ErrorAction SilentlyContinue
}
function Ok([string]$m) { Log "OK  $m" "Green" }
function Warn([string]$m) { Log "!   $m" "Yellow" }
function Step([string]$m) { Log "==> $m" "Cyan" }

function Register-CouchlinkElevatedUpnp {
    $taskName = "CouchlinkElevatedUpnp"
    $stable = Join-Path $RunDir "enable-upnp.ps1"
    if ($ScriptSelf -and (Test-Path $ScriptSelf)) {
        Copy-Item -Force $ScriptSelf $stable -ErrorAction SilentlyContinue
    }
    $target = if (Test-Path $stable) { $stable } else { $ScriptSelf }
    Unregister-ScheduledTask -TaskName $taskName -Confirm:$false -ErrorAction SilentlyContinue
    $action = New-ScheduledTaskAction -Execute "powershell.exe" `
        -Argument "-NoProfile -ExecutionPolicy Bypass -WindowStyle Minimized -File `"$target`""
    $trigger = New-ScheduledTaskTrigger -Once -At ([datetime]"2000-01-01T00:00:00")
    $principal = New-ScheduledTaskPrincipal -UserId $env:USERNAME -LogonType Interactive -RunLevel Highest
    $settings = New-ScheduledTaskSettingsSet -AllowStartIfOnBatteries -DontStopIfGoingOnBatteries `
        -StartWhenAvailable -ExecutionTimeLimit (New-TimeSpan -Minutes 5)
    Register-ScheduledTask -TaskName $taskName -Action $action -Trigger $trigger `
        -Principal $principal -Settings $settings -Force | Out-Null
    try {
        $t = Get-ScheduledTask -TaskName $taskName
        $t.Triggers | ForEach-Object { $_.Enabled = $false }
        Set-ScheduledTask -InputObject $t | Out-Null
    } catch {}
    Ok "registered $taskName (later --online: no UAC)"
}

function Finish([int]$code) {
    Set-Content -Path $Marker -Value $code -Encoding ASCII
    exit $code
}

Set-Content -Path $LogFile -Value ("Couchlink enable-upnp " + (Get-Date -Format o)) -Encoding utf8
try { Register-CouchlinkElevatedUpnp } catch { Warn ("elevated task: " + $_.Exception.Message) }

Step "Couchlink: Windows UPnP prep (Private network + discovery + services)"

$profiles = @(Get-NetConnectionProfile -ErrorAction SilentlyContinue |
    Where-Object {
        $_.InterfaceAlias -notmatch 'WSL|vEthernet|Loopback|Bluetooth' -and
        $_.IPv4Connectivity -ne 'Disconnected'
    })
if (-not $profiles) {
    Warn "No active LAN/Wi-Fi profile found"
} else {
    foreach ($p in $profiles) {
        if ($p.NetworkCategory -eq "Private") {
            Ok ("already Private: {0} ({1})" -f $p.Name, $p.InterfaceAlias)
            continue
        }
        try {
            Set-NetConnectionProfile -InterfaceIndex $p.InterfaceIndex -NetworkCategory Private -ErrorAction Stop
            Ok ("set Private: {0} ({1}) was {2}" -f $p.Name, $p.InterfaceAlias, $p.NetworkCategory)
        } catch {
            Warn ("could not set Private on {0}: {1}" -f $p.InterfaceAlias, $_.Exception.Message)
            Warn "If Group Policy locks this, set it in Settings → Network → Properties → Private"
        }
    }
}

foreach ($g in @("Network Discovery")) {
    try {
        Enable-NetFirewallRule -DisplayGroup $g -ErrorAction SilentlyContinue | Out-Null
        Get-NetFirewallRule -DisplayGroup $g -ErrorAction SilentlyContinue | ForEach-Object {
            try { Set-NetFirewallRule -Name $_.Name -Profile Private,Domain -Enabled True -ErrorAction SilentlyContinue } catch {}
        }
        Ok "firewall group enabled: $g"
    } catch {
        Warn ("firewall group $g : " + $_.Exception.Message)
    }
}

foreach ($svcName in @("SSDPSRV", "upnphost")) {
    try {
        $svc = Get-Service -Name $svcName -ErrorAction Stop
        if ($svc.StartType -eq "Disabled") {
            Set-Service -Name $svcName -StartupType Manual -ErrorAction Stop
        }
        if ($svc.Status -ne "Running") {
            Start-Service -Name $svcName -ErrorAction Stop
        }
        Ok ("service running: $svcName")
    } catch {
        Warn ("service $svcName : " + $_.Exception.Message)
    }
}

Start-Sleep -Seconds 2

if (-not $LanIp) {
    $LanIp = (Get-NetIPAddress -AddressFamily IPv4 |
        Where-Object { $_.IPAddress -match '^192\.168\.|^10\.' -and $_.InterfaceAlias -notmatch 'WSL|vEthernet|Loopback' } |
        Select-Object -First 1).IPAddress
}

try {
    $nat = New-Object -ComObject HNetCfg.NATUPnP
    $maps = $nat.StaticPortMappingCollection
    if ($null -eq $maps) {
        Warn "Still no UPnP IGD (StaticPortMappingCollection is null)"
        Log "Windows prepared; enable UPnP/IGD on the gateway (often http://192.168.1.1)." "Yellow"
        Log "Friends may need manual port-forward TCP 8443 + UDP/TCP 3478 until then." "DarkGray"
        if ($SkipMap) { Finish 0 }
        Finish 2
    }
    Ok ("UPnP IGD visible (existing maps: {0})" -f $maps.Count)
} catch {
    Warn ("NATUPnP COM: " + $_.Exception.Message)
    Finish 1
}

if ($SkipMap -or -not $LanIp) {
    if (-not $LanIp) { Warn "No LAN IP for mapping" }
    Finish 0
}

function Add-Map([int]$Port, [string]$Proto, [string]$Name) {
    try {
        try { $maps.Remove($Port, $Proto) | Out-Null } catch {}
        $maps.Add($Port, $Proto, $Port, $LanIp, $true, $Name) | Out-Null
        Ok ("mapped $Port/$Proto -> $LanIp ($Name)")
        return $true
    } catch {
        Warn ("map $Port/$Proto failed: " + $_.Exception.Message)
        return $false
    }
}

$ok8443 = Add-Map 8443 "TCP" "couchlink-signaling"
Add-Map 3478 "TCP" "couchlink-turn-tcp" | Out-Null
Add-Map 3478 "UDP" "couchlink-turn-udp" | Out-Null

if ($ok8443) { Finish 0 }
Finish 1
