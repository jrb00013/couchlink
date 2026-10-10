# Dump guest memory from a running RPCS3 through its GDB stub (127.0.0.1:2345).
# Run on WINDOWS (the stub only listens on Windows loopback, WSL cannot reach it):
#   powershell.exe -NoProfile -ExecutionPolicy Bypass -File gdb_dump.ps1 -Out C:\Users\josep\bo2dump
# WARNING: RPCS3's GDB stub PAUSES the whole emulator the moment a client connects
# ("Emulation is being paused... Got connection"). Closing the socket without
# telling it to continue leaves the game frozen (this happened live). The script
# therefore always sends 'c' (continue) in a finally block. If a game ever stays
# paused anyway: RPCS3 menu Emulation > Resume. The stub is single-client and can
# refuse later connections, so plan on ONE dump per game boot.
param(
  [string]$Out = "$env:TEMP\bo2dump",
  [int]$Port = 2345,
  # name = "start,length" (hex). Defaults cover the whole story from the issue.
  [hashtable]$Ranges = @{
    "wait_loop"   = "73d000,1000"   # main-thread spin 0x73d6ec..0x73d7b4 + 0x73d09c callee
    "wait_caller" = "3ac9c0,100"    # 0x3aca08 caller
    "hang_detect" = "41c3c0,80"     # watchdog thread (already patched)
    "ccc740"      = "ccc700,80"     # object polled by bl 0x73d09c(*(0xccc740),0,1,r30)
    "heartbeat"   = "cbcd00,20"     # last-heartbeat word at 0xcbcd08
  }
)
$ErrorActionPreference = "Stop"
New-Item -ItemType Directory -Force -Path $Out | Out-Null
$c = New-Object System.Net.Sockets.TcpClient("127.0.0.1", $Port)
$s = $c.GetStream(); $s.ReadTimeout = 5000
function Chk([string]$d) { $t = 0; foreach ($b in [Text.Encoding]::ASCII.GetBytes($d)) { $t += $b }; "{0:x2}" -f ($t % 256) }
function Send([string]$d) {
  $p = [Text.Encoding]::ASCII.GetBytes("`$$d#$(Chk $d)"); $s.Write($p, 0, $p.Length); $s.Flush()
  $sb = New-Object Text.StringBuilder; $n = 0
  while ($true) {
    $x = $s.ReadByte(); if ($x -lt 0) { throw "stub closed" }
    $ch = [char]$x
    if ($ch -eq '+' -and $sb.Length -eq 0) { continue }
    [void]$sb.Append($ch)
    if ($ch -eq '#') { $n = 2 }
    elseif ($n -gt 0) { $n--; if ($n -eq 0) { break } }
  }
  $r = $sb.ToString(); $b2 = [Text.Encoding]::ASCII.GetBytes("+"); $s.Write($b2, 0, 1)
  $r.TrimStart('$').Substring(0, $r.Length - 4)
}
try {
"stub says: " + (Send "qSupported")
foreach ($k in $Ranges.Keys) {
  $a, $l = $Ranges[$k].Split(','); $addr = [Convert]::ToInt64($a, 16); $len = [Convert]::ToInt32($l, 16)
  $hex = New-Object Text.StringBuilder
  for ($o = 0; $o -lt $len; $o += 0x200) {
    $n = [Math]::Min(0x200, $len - $o)
    $r = Send ("m{0:x},{1:x}" -f ($addr + $o), $n)
    if ($r -cmatch '^E[0-9A-F]{2}$') { throw "read error $r at $('{0:x}' -f ($addr+$o)) ($k)" }
    [void]$hex.Append($r)
  }
  Set-Content -NoNewline -Path "$Out\$k.$a.hex" -Value $hex.ToString()
  "{0}: {1} bytes @ 0x{2}" -f $k, ($hex.Length / 2), $a
}
} finally {
  # Resume the emulator no matter what happened above.
  try { $p = [Text.Encoding]::ASCII.GetBytes("`$c#63"); $s.Write($p, 0, $p.Length); $s.Flush(); Start-Sleep -Milliseconds 300 } catch {}
  $c.Close()
}
