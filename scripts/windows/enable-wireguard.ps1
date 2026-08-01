#Requires -RunAsAdministrator
# Install/activate the couchlink WireGuard tunnel on Windows and open UDP 51820.
# Registers CouchlinkElevatedWireGuard for later no-UAC runs (same pattern as UPnP).
#
# Exit: 0 = tunnel service installed/running
#       1 = hard failure
param(
    [Parameter(Mandatory = $true)]
    [string]$ConfPath,
    [string]$TunnelName = "couchlink",
    [string]$RunDir = "",
    [int]$ListenPort = 51820
)

$ErrorActionPreference = "Continue"
$RunDir = if ($RunDir) { $RunDir } else { Join-Path $env:LOCALAPPDATA "couchlink-run" }
New-Item -ItemType Directory -Force -Path $RunDir | Out-Null
$Marker = Join-Path $RunDir "enable-wireguard.exit"
$LogFile = Join-Path $RunDir "enable-wireguard.log"
$ScriptSelf = if ($PSCommandPath) { $PSCommandPath } else { $MyInvocation.MyCommand.Path }
$WgExe = Join-Path ${env:ProgramFiles} "WireGuard\wireguard.exe"
$WgCli = Join-Path ${env:ProgramFiles} "WireGuard\wg.exe"

function Log([string]$m, [string]$color = "White") {
    Write-Host $m -ForegroundColor $color
    Add-Content -Path $LogFile -Value $m -Encoding utf8 -ErrorAction SilentlyContinue
}
function Ok([string]$m) { Log "OK  $m" "Green" }
function Warn([string]$m) { Log "!   $m" "Yellow" }
function Step([string]$m) { Log "==> $m" "Cyan" }

function Register-CouchlinkElevatedWireGuard {
    $taskName = "CouchlinkElevatedWireGuard"
    $stable = Join-Path $RunDir "enable-wireguard.ps1"
    if ($ScriptSelf -and (Test-Path $ScriptSelf)) {
        Copy-Item -Force $ScriptSelf $stable -ErrorAction SilentlyContinue
    }
    $target = if (Test-Path $stable) { $stable } else { $ScriptSelf }
    # Persist last ConfPath for schtasks re-runs.
    $argsFile = Join-Path $RunDir "enable-wireguard.args"
    Set-Content -Path $argsFile -Value $ConfPath -Encoding ASCII
    Unregister-ScheduledTask -TaskName $taskName -Confirm:$false -ErrorAction SilentlyContinue
    $action = New-ScheduledTaskAction -Execute "powershell.exe" `
        -Argument "-NoProfile -ExecutionPolicy Bypass -WindowStyle Minimized -File `"$target`" -ConfPath `"$ConfPath`" -TunnelName `"$TunnelName`" -ListenPort $ListenPort"
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
    Ok "registered $taskName (later bring-up: no UAC)"
}

function Finish([int]$code) {
    Set-Content -Path $Marker -Value $code -Encoding ASCII
    exit $code
}

Set-Content -Path $LogFile -Value ("Couchlink enable-wireguard " + (Get-Date -Format o)) -Encoding utf8

if (-not (Test-Path $WgExe)) {
    Warn "WireGuard for Windows not found at $WgExe"
    Warn "Install from https://www.wireguard.com/install/ then re-run"
    Finish 1
}
if (-not (Test-Path $ConfPath)) {
    Warn "conf missing: $ConfPath"
    Finish 1
}

try { Register-CouchlinkElevatedWireGuard } catch { Warn ("elevated task: " + $_.Exception.Message) }

Step "Firewall UDP $ListenPort (WireGuard handshake)"
$name = "couchlink-wireguard-$ListenPort"
if (-not (Get-NetFirewallRule -DisplayName $name -ErrorAction SilentlyContinue)) {
    New-NetFirewallRule -DisplayName $name -Direction Inbound -Protocol UDP `
        -LocalPort $ListenPort -Action Allow -Profile Any -ErrorAction SilentlyContinue | Out-Null
    Ok "firewall allow UDP $ListenPort"
} else {
    Ok "firewall rule already present"
}

$dest = Join-Path $RunDir "$TunnelName.conf"
Copy-Item -Force $ConfPath $dest
Ok "conf -> $dest"

Step "Install / restart WireGuard tunnel service ($TunnelName)"
# Uninstall existing service if present (idempotent refresh).
& $WgExe /uninstalltunnelservice $TunnelName 2>$null | Out-Null
Start-Sleep -Milliseconds 400
& $WgExe /installtunnelservice $dest
$installEc = $LASTEXITCODE

# Wait for interface address / service
$deadline = (Get-Date).AddSeconds(20)
$ip = $null
$svcOk = $false
while ((Get-Date) -lt $deadline) {
    Start-Sleep -Milliseconds 500
    $svc = Get-Service -Name "WireGuardTunnel`$$TunnelName" -ErrorAction SilentlyContinue
    if ($svc -and ($svc.Status -eq "Running" -or $svc.Status -eq "StartPending")) {
        $svcOk = $true
    }
    $ip = Get-NetIPAddress -AddressFamily IPv4 -ErrorAction SilentlyContinue |
        Where-Object {
            $_.InterfaceAlias -eq $TunnelName -or
            $_.InterfaceAlias -match 'WireGuard|Wintun' -or
            $_.IPAddress -eq '10.66.0.1'
        } |
        Select-Object -First 1 -ExpandProperty IPAddress
    if ($ip) { break }
    if (Test-Path $WgCli) {
        $show = & $WgCli show $TunnelName 2>$null
        if ($show) { Ok "wg show $TunnelName ok"; break }
    }
}

if (-not $ip -and -not $svcOk -and $installEc -ne 0) {
    Warn "installtunnelservice exit $installEc and tunnel not visible"
    Finish 1
}
if ($installEc -ne 0 -and ($ip -or $svcOk)) {
    Warn "installtunnelservice exit $installEc but tunnel is present — continuing"
}

# WSL portproxy so mesh IP :8443/:3478 reaches WSL listeners (best-effort).
$wslIp = $null
try {
    $wslIp = (wsl -e bash -lc "ip -4 -o addr show eth0 2>/dev/null | awk '{print \$4}' | cut -d/ -f1 | head -1" 2>$null)
    $wslIp = ($wslIp | Out-String).Trim()
} catch {}
if ($wslIp -match '^\d+\.\d+\.\d+\.\d+$') {
    Step "WSL portproxy 8443/3478 -> $wslIp (mesh + public)"
    foreach ($port in @(8443, 3478)) {
        netsh interface portproxy delete v4tov4 listenaddress=0.0.0.0 listenport=$port 2>$null | Out-Null
        netsh interface portproxy add v4tov4 listenaddress=0.0.0.0 listenport=$port connectaddress=$wslIp connectport=$port | Out-Null
    }
    Ok "portproxy ready"
} else {
    Warn "could not detect WSL IP for portproxy"
}

Ok "WireGuard tunnel '$TunnelName' installed"
Finish 0
