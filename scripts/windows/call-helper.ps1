#Requires -Version 5.1
<#
.SYNOPSIS
  Non-elevated client for the Couchlink Helper named pipe (no UAC).

.EXAMPLE
  .\call-helper.ps1 -Op ping
  .\call-helper.ps1 -Op online_prep -SkipMap -WslIp 172.18.0.2
  .\call-helper.ps1 -Op firewall_unblock
#>
param(
    [Parameter(Mandatory = $true)]
    [ValidateSet('ping', 'online_prep', 'firewall_unblock')]
    [string]$Op,

    [switch]$SkipMap,
    [string]$WslIp = "",
    [int]$SignalingPort = 8443,
    [int]$TurnPort = 3478,

    [int]$TimeoutMs = 120000
)

$ErrorActionPreference = "Stop"
$PipeName = "couchlink-helper"

switch ($Op) {
    'ping' { $req = '{"op":"ping"}' }
    'firewall_unblock' { $req = '{"op":"firewall_unblock"}' }
    'online_prep' {
        $skip = if ($SkipMap) { 'true' } else { 'false' }
        $ipJson = if ($WslIp -and $WslIp.Trim()) {
            '"wsl_ip":"' + ($WslIp.Trim() -replace '\\', '\\\\' -replace '"', '') + '",'
        } else { '' }
        $req = '{"op":"online_prep","skip_map":' + $skip + ',' + $ipJson +
            '"signaling_port":' + $SignalingPort + ',"turn_port":' + $TurnPort + '}'
    }
}

try {
    $client = New-Object System.IO.Pipes.NamedPipeClientStream('.', $PipeName, [System.IO.Pipes.PipeDirection]::InOut)
    $client.Connect($TimeoutMs)
} catch {
    Write-Error ("helper pipe connect failed (is CouchlinkHelper service running?): " + $_.Exception.Message)
    exit 1
}

try {
    $utf8 = New-Object System.Text.UTF8Encoding $false
    $bytes = $utf8.GetBytes($req + "`n")
    $client.Write($bytes, 0, $bytes.Length)
    $client.Flush()

    $buf = New-Object byte[] 4096
    $ms = New-Object System.IO.MemoryStream
    while ($true) {
        $n = $client.Read($buf, 0, $buf.Length)
        if ($n -le 0) { break }
        $ms.Write($buf, 0, $n)
        $soFar = $utf8.GetString($ms.ToArray())
        if ($soFar.Contains("`n")) { break }
    }
    $line = ($utf8.GetString($ms.ToArray()) -split "`n")[0].Trim()
    Write-Output $line

    if (-not $line) {
        exit 1
    }
    $json = $line | ConvertFrom-Json
    if (-not $json.ok) {
        if ($null -ne $json.exit) { exit [int]$json.exit }
        exit 1
    }
    if ($null -ne $json.exit) {
        exit [int]$json.exit
    }
    exit 0
} finally {
    if ($client) { $client.Dispose() }
}
