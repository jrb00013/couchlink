# Prepare Windows for couchlink --online:
#   Private profile, discovery, UPnP services, NATUPnP maps,
#   firewall allow for 8443/3478, and WSL portproxy (IPv4+IPv6 → WSL).
# Registers CouchlinkElevatedUpnp so later --online runs need no UAC.
# Invoked by Couchlink Helper (LocalSystem) or elevated UAC/task — no #Requires
# so LocalSystem is not rejected by PowerShell's RunAsAdministrator check.
#
# Exit: 0 = IGD OK / maps applied (or -SkipMap prep done)
#       2 = Windows prepared (portproxy/firewall OK) but router IGD still missing
#       1 = hard failure
param(
    [switch]$SkipMap,
    [string]$LanIp = "",
    [string]$WslIp = "",
    [string]$RunDir = "",
    [int]$SignalingPort = 8443,
    [int]$TurnPort = 3478
)

$ErrorActionPreference = "Continue"
$RunDir = if ($RunDir) { $RunDir } else { Join-Path $env:LOCALAPPDATA "couchlink-run" }
New-Item -ItemType Directory -Force -Path $RunDir | Out-Null
$Marker = Join-Path $RunDir "enable-upnp.exit"
$LogFile = Join-Path $RunDir "enable-upnp.log"
$Ipv6File = Join-Path $RunDir "public-ipv6.txt"
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

function Get-WslConnectIp {
    if ($WslIp -and $WslIp.Trim()) { return $WslIp.Trim() }
    try {
        $out = & wsl.exe -e sh -c "ip -4 -o addr show eth0 2>/dev/null | awk '{print `$4}' | cut -d/ -f1 | head -1" 2>$null
        $ip = ("$out").Trim()
        if ($ip -match '^\d+\.\d+\.\d+\.\d+$') { return $ip }
    } catch {}
    try {
        $out = & wsl.exe -e sh -c "hostname -I | awk '{print `$1}'" 2>$null
        $ip = ("$out").Trim()
        if ($ip -match '^\d+\.\d+\.\d+\.\d+$') { return $ip }
    } catch {}
    return $null
}

function Ensure-Fw([string]$name, [string]$proto, [int]$port) {
    if (-not (Get-NetFirewallRule -DisplayName $name -ErrorAction SilentlyContinue)) {
        New-NetFirewallRule -DisplayName $name -Direction Inbound -Protocol $proto `
            -LocalPort $port -Action Allow -Profile Any | Out-Null
        Ok "firewall allow $name ($proto/$port)"
    } else {
        Ok "firewall exists $name"
    }
}

function Set-PortProxy([string]$listenFamily, [string]$listenAddr, [int]$listenPort, [string]$connectAddr, [int]$connectPort) {
    $del = "interface portproxy delete $listenFamily listenaddress=$listenAddr listenport=$listenPort"
    $add = "interface portproxy add $listenFamily listenaddress=$listenAddr listenport=$listenPort connectaddress=$connectAddr connectport=$connectPort"
    cmd.exe /c "netsh $del" 2>$null | Out-Null
    $r = cmd.exe /c "netsh $add" 2>&1
    if ($LASTEXITCODE -ne 0) {
        Warn "portproxy $listenFamily :$listenPort failed: $r"
        return $false
    }
    Ok "portproxy $listenFamily $listenAddr`:$listenPort → $connectAddr`:$connectPort"
    return $true
}

function Write-PublicIpv6 {
    Remove-Item -Force $Ipv6File -ErrorAction SilentlyContinue
    $candidates = @(Get-NetIPAddress -AddressFamily IPv6 -ErrorAction SilentlyContinue |
        Where-Object {
            $_.AddressState -eq 'Preferred' -and
            $_.InterfaceAlias -notmatch 'WSL|vEthernet|Loopback|Bluetooth|Default Switch' -and
            $_.IPAddress -match '^[23]' -and
            $_.IPAddress -notmatch '^fd' -and
            $_.IPAddress -notmatch '^fe80'
        })
    if (-not $candidates) { return }
    # Prefer DHCP / stable over temporary privacy addresses.
    $best = $candidates |
        Sort-Object @{
            Expression = {
                if ($_.PrefixOrigin -eq 'Dhcp') { 0 }
                elseif ($_.SuffixOrigin -eq 'Link') { 1 }
                else { 2 }
            }
        } |
        Select-Object -First 1
    if ($best) {
        Set-Content -Path $Ipv6File -Value $best.IPAddress -Encoding ASCII
        Ok ("public IPv6 for invite: {0}" -f $best.IPAddress)
    }
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

Step "Couchlink: firewall + WSL portproxy (no router needed for LAN/IPv6 path)"
Ensure-Fw "couchlink-signaling-$SignalingPort" "TCP" $SignalingPort
Ensure-Fw "couchlink-turn-$TurnPort-tcp" "TCP" $TurnPort
Ensure-Fw "couchlink-turn-$TurnPort-udp" "UDP" $TurnPort
# Headscale control plane + embedded DERP STUN (friend mesh join).
Ensure-Fw "couchlink-headscale-8080" "TCP" 8080
Ensure-Fw "couchlink-derp-stun-34790" "UDP" 34790

$connectIp = Get-WslConnectIp
if ($connectIp) {
    Set-PortProxy "v4tov4" "0.0.0.0" $SignalingPort $connectIp $SignalingPort | Out-Null
    Set-PortProxy "v4tov4" "0.0.0.0" $TurnPort $connectIp $TurnPort | Out-Null
    # Headscale control + DERP STUN (friend mesh join).
    Set-PortProxy "v4tov4" "0.0.0.0" 8080 $connectIp 8080 | Out-Null
    Set-PortProxy "v4tov4" "0.0.0.0" 34790 $connectIp 34790 | Out-Null
    Set-PortProxy "v6tov4" "::" $SignalingPort $connectIp $SignalingPort | Out-Null
    Set-PortProxy "v6tov4" "::" $TurnPort $connectIp $TurnPort | Out-Null
    Set-PortProxy "v6tov4" "::" 8080 $connectIp 8080 | Out-Null
} else {
    Warn "WSL IP not found — skipped portproxy (friends may not reach WSL listeners)"
}

Write-PublicIpv6

Start-Sleep -Seconds 1

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
        Log "Portproxy/firewall ready. Router UPnP off — IPv6 invite or tunnel fallback covers friends." "Yellow"
        if ($SkipMap) { Finish 0 }
        Finish 2
    }
    Ok ("UPnP IGD visible (existing maps: {0})" -f $maps.Count)
} catch {
    Warn ("NATUPnP COM: " + $_.Exception.Message)
    # Portproxy still applied — not a hard fail for --online.
    Finish 2
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

$ok8443 = Add-Map $SignalingPort "TCP" "couchlink-signaling"
Add-Map $TurnPort "TCP" "couchlink-turn-tcp" | Out-Null
Add-Map $TurnPort "UDP" "couchlink-turn-udp" | Out-Null
# Without these the friend can never reach the Headscale control plane.
Add-Map 8080 "TCP" "couchlink-headscale" | Out-Null
Add-Map 34790 "UDP" "couchlink-derp-stun" | Out-Null

if ($ok8443) { Finish 0 }
Finish 2
