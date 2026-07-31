# Try router port-forwards via Windows NATUPnP COM (WSL miniupnpc often can't see the IGD).
# Exit 0 if at least TCP 8443 was mapped; exit 1 if UPnP unavailable/failed.
param(
    [string]$LanIp = ""
)

$ErrorActionPreference = "Stop"

if (-not $LanIp) {
    $LanIp = (Get-NetIPAddress -AddressFamily IPv4 |
        Where-Object { $_.IPAddress -match '^192\.168\.|^10\.' -and $_.InterfaceAlias -notmatch 'WSL|vEthernet|Loopback' } |
        Select-Object -First 1).IPAddress
}
if (-not $LanIp) {
    Write-Host "UPnP: no LAN IP found" -ForegroundColor Yellow
    exit 1
}

try {
    $nat = New-Object -ComObject HNetCfg.NATUPnP
    $maps = $nat.StaticPortMappingCollection
} catch {
    Write-Host "UPnP: COM unavailable ($($_.Exception.Message))" -ForegroundColor Yellow
    exit 1
}
if ($null -eq $maps) {
    Write-Host "UPnP: disabled on this router/PC (StaticPortMappingCollection is null)" -ForegroundColor Yellow
    exit 1
}

function Add-Map([int]$Port, [string]$Proto, [string]$Name) {
    try {
        try { $maps.Remove($Port, $Proto) | Out-Null } catch {}
        $maps.Add($Port, $Proto, $Port, $LanIp, $true, $Name) | Out-Null
        Write-Host "UPnP: opened $Port/$Proto -> $LanIp ($Name)" -ForegroundColor Green
        return $true
    } catch {
        Write-Host "UPnP: failed $Port/$Proto ($($_.Exception.Message))" -ForegroundColor Yellow
        return $false
    }
}

$ok8443 = Add-Map 8443 "TCP" "couchlink-signaling"
Add-Map 3478 "TCP" "couchlink-turn-tcp" | Out-Null
Add-Map 3478 "UDP" "couchlink-turn-udp" | Out-Null

if ($ok8443) { exit 0 }
exit 1
