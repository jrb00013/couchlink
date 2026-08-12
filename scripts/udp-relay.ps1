param(
  [int]$ListenPort = 3478,
  [string]$TargetHost = "127.0.0.1",
  [int]$TargetPort = 3478
)

# WSL2 NAT mode does not route inbound UDP addressed to a real external
# interface (LAN IP, public IPv6) into the WSL VM — only TCP/UDP to
# 127.0.0.1 gets bridged in (localhostForwarding). But turnserver inside
# WSL also binds 127.0.0.1, and that loopback hop DOES work both ways.
# So: listen here on Windows' externally-reachable addresses, and forward
# each datagram to the WSL turnserver via loopback, matching responses
# back to whichever client sent the request that triggered them.
#
# One dedicated local socket per client (not a single shared one) — a STUN/
# TURN response doesn't self-identify which client it's for, so sharing one
# outbound socket across concurrent clients would let their replies cross.

$publicV6 = New-Object System.Net.Sockets.UdpClient([System.Net.Sockets.AddressFamily]::InterNetworkV6)
$publicV6.Client.DualMode = $true
$publicV6.Client.Bind((New-Object System.Net.IPEndPoint([System.Net.IPAddress]::IPv6Any, $ListenPort)))

Write-Output "==> udp-relay: listening on [::]:$ListenPort (dual-stack) -> ${TargetHost}:${TargetPort}"

$sessions = @{}  # client endpoint string -> per-client UdpClient connected to target

while ($true) {
  $clientEP = New-Object System.Net.IPEndPoint([System.Net.IPAddress]::IPv6Any, 0)
  try {
    $data = $publicV6.Receive([ref]$clientEP)
  } catch {
    Start-Sleep -Milliseconds 200
    continue
  }

  $key = $clientEP.ToString()
  if (-not $sessions.ContainsKey($key)) {
    $targetSocket = New-Object System.Net.Sockets.UdpClient(0)
    $targetSocket.Connect($TargetHost, $TargetPort)
    $sessions[$key] = $targetSocket

    # Per-client async listener: whatever the target sends back for this
    # client's socket goes straight back out to that client's real address.
    $state = [PSCustomObject]@{ Socket = $targetSocket; Client = $clientEP; Public = $publicV6 }
    $callback = {
      param($ar)
      $s = $ar.AsyncState
      try {
        $remote = New-Object System.Net.IPEndPoint([System.Net.IPAddress]::Any, 0)
        $resp = $s.Socket.EndReceive($ar, [ref]$remote)
        $s.Public.Send($resp, $resp.Length, $s.Client) | Out-Null
        $s.Socket.BeginReceive($callback, $s) | Out-Null
      } catch {
        # target session ended (timeout/close) — drop it silently
      }
    }
    $targetSocket.BeginReceive($callback, $state) | Out-Null
  }

  $sessions[$key].Send($data, $data.Length) | Out-Null
}
