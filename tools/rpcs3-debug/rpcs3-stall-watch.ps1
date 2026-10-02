# Windows-native stall watcher: no WSL involved, so it keeps working when WSL<->Windows interop times out.
#   powershell -NoProfile -ExecutionPolicy Bypass -File rpcs3-stall-watch.ps1 [-StallSecs 30] [-Quiet]
# Run from a LOCAL Windows folder (copy this script, threads.ps1 and screenshot.ps1 there; start-windows-watcher.sh does it).
# On >= StallSecs of game-event silence it writes <OutRoot>\<timestamp>\ with: log-slice.txt, threads.txt,
# a small minidump (+names.csv), a screenshot, the guest thread dump (via dump_threads.trigger) and result.txt.
param(
  [string]$RpcsDir = "C:\Users\josep\RPCS3",
  [string]$OutRoot = "$env:USERPROFILE\rpcs3-stalls",
  [int]$StallSecs = 30
)
$ErrorActionPreference = "Continue"
$here = Split-Path -Parent $MyInvocation.MyCommand.Path
$log = Join-Path $RpcsDir "log\RPCS3.log"
New-Item -ItemType Directory -Force -Path $OutRoot | Out-Null
$noise = 'cellAudio|cellMic|sys_event|sys_mmapper|DualSense|Performance|Syscall Usage|PERF:|RSX: (Add program|Program compiled)|SPU: (Building|New SPU block)'
# The log line marker is U+00B7. Windows PowerShell reads a BOM-less script as ANSI, so never put it in source.
$dot = [regex]::Escape([string][char]0xB7)
$lineRe = '^' + $dot + '[A-Z!] (\d+):(\d+):([\d.]+) '
function Get-Tail([string]$path, [int]$bytes) {
  $fs = [System.IO.File]::Open($path, 'Open', 'Read', 'ReadWrite')
  try { $n = [Math]::Min($bytes, $fs.Length); [void]$fs.Seek(-$n, 'End'); $sr = New-Object System.IO.StreamReader($fs, [Text.Encoding]::UTF8); $sr.ReadToEnd() } finally { $fs.Close() }
}
function Get-Silence {
  try { $t = Get-Tail $log 3000000 } catch { return 0 }
  $lastAny = $null; $lastGame = $null
  foreach ($l in ($t -split "`n")) {
    if ($l -match $lineRe) {
      $s = [int]$Matches[1]*3600 + [int]$Matches[2]*60 + [double]$Matches[3]
      $lastAny = $s
      if ($l -notmatch $noise) { $lastGame = $s }
    }
  }
  if ($null -eq $lastAny -or $null -eq $lastGame) { return 0 }
  [int]($lastAny - $lastGame)
}
"$(Get-Date -f HH:mm:ss) native watcher: $log (stall >= ${StallSecs}s) -> $OutRoot"
$episode = $null; $started = $null
while ($true) {
  Start-Sleep -Seconds 5
  if (-not (Test-Path $log)) { continue }
  $s = Get-Silence
  if (-not $episode -and $s -ge $StallSecs) {
    $episode = Join-Path $OutRoot (Get-Date -f yyyyMMdd-HHmmss); $started = Get-Date
    New-Item -ItemType Directory -Force -Path $episode | Out-Null
    "$(Get-Date -f HH:mm:ss) STALL (silence ${s}s) -> $episode"
    # 1) guest thread dump via the trigger (no pause), copy the new block
    try { $before = ([regex]::Matches((Get-Tail $log 40000000), 'Guest thread dump requested')).Count } catch { $before = 0 }
    New-Item -ItemType File -Force -Path (Join-Path $RpcsDir "dump_threads.trigger") | Out-Null
    for ($i = 0; $i -lt 10; $i++) {
      Start-Sleep -Seconds 1
      try { $now = ([regex]::Matches((Get-Tail $log 40000000), 'Guest thread dump requested')).Count } catch { $now = $before }
      if ($now -gt $before) { break }
    }
    try {
      $t = Get-Tail $log 40000000; $k = $t.LastIndexOf('Guest thread dump requested')
      if ($k -ge 0) { $t.Substring($k) | Set-Content -Encoding UTF8 (Join-Path $episode "guest-threads.txt") }
      else { "no dump block: this RPCS3 build has no dump_threads.trigger" | Set-Content (Join-Path $episode "guest-threads.txt") }
      ($t -split "`n" | Where-Object { $_ -notmatch 'DualSense|Performance|PERF:' } | Select-Object -Last 600) -join "`n" | Set-Content -Encoding UTF8 (Join-Path $episode "log-slice.txt")
    } catch {}
    # 2) native threads + small minidump (+names) , 3) screenshot
    & (Join-Path $here "threads.ps1") -Seconds 5 -Top 20 -Dump (Join-Path $episode "rpcs3.dmp") *>&1 | Out-File -Encoding UTF8 (Join-Path $episode "threads.txt")
    & (Join-Path $here "screenshot.ps1") -Out (Join-Path $episode "screenshot.png") | Out-Null
    "$(Get-Date -f HH:mm:ss) evidence captured"
  }
  elseif ($episode -and $s -lt 10) {
    $el = [int]((Get-Date) - $started).TotalSeconds
    "stall lasted ~${el}s wall (incl. capture)" | Set-Content (Join-Path $episode "result.txt")
    "$(Get-Date -f HH:mm:ss) stall ended after ~${el}s"
    $episode = $null
  }
}
