# Per-thread CPU deltas (with thread names) and optional full minidump for a live rpcs3.exe.
# Run on WINDOWS:  powershell -NoProfile -ExecutionPolicy Bypass -File threads.ps1 [-Seconds 5] [-Dump C:\path\rpcs3.dmp]
# Non-invasive: reads counters only. -Dump writes a SMALL minidump (thread stacks + registers, ~100 MB,
# a few seconds of pause). -Full adds all memory: for RPCS3 that is 30+ GB, takes minutes and keeps the
# game SUSPENDED the whole time (learned the hard way 2026-10-01) - only use it on an already-dead process.
param([int]$Seconds = 5, [string]$Dump = "", [int]$Top = 25, [switch]$Full, [double]$Nudge = 0)
$ErrorActionPreference = "Stop"
Add-Type @"
using System; using System.Runtime.InteropServices;
public class TN {
  [DllImport("kernel32.dll", SetLastError=true)] static extern IntPtr OpenThread(uint access, bool inherit, uint id);
  [DllImport("kernel32.dll")] static extern bool CloseHandle(IntPtr h);
  [DllImport("kernel32.dll")] static extern int GetThreadDescription(IntPtr h, out IntPtr name);
  [DllImport("kernel32.dll")] static extern IntPtr LocalFree(IntPtr p);
  public static string Name(uint tid) {
    IntPtr h = OpenThread(0x0800 /*THREAD_QUERY_LIMITED_INFORMATION*/, false, tid);
    if (h == IntPtr.Zero) return "<open failed " + Marshal.GetLastWin32Error() + ">";
    try { IntPtr p; if (GetThreadDescription(h, out p) < 0 || p == IntPtr.Zero) return "<none>";
          string s = Marshal.PtrToStringUni(p); LocalFree(p); return s; }
    finally { CloseHandle(h); }
  }
}
"@
$p = Get-Process rpcs3 -ErrorAction Stop
if ($Nudge -gt 0) {
  # Suspend every thread for $Nudge seconds, then resume. Hypothesis (2 observations, unproven): MK cutscene-end
  # stalls ended right after the process was suspended/resumed. Resume ALWAYS runs (finally).
  Add-Type -MemberDefinition '[DllImport("ntdll.dll")] public static extern int NtSuspendProcess(IntPtr h); [DllImport("ntdll.dll")] public static extern int NtResumeProcess(IntPtr h);' -Name NT -Namespace W
  try { [W.NT]::NtSuspendProcess($p.Handle) | Out-Null; Start-Sleep -Milliseconds ([int]($Nudge * 1000)) }
  finally { [W.NT]::NtResumeProcess($p.Handle) | Out-Null }
  "nudged: suspended $Nudge s then resumed"; return
}
function Snap { $h=@{}; $p.Refresh(); foreach ($t in $p.Threads) { try { $h[$t.Id] = $t.TotalProcessorTime.TotalMilliseconds } catch {} }; $h }
$a = Snap; Start-Sleep -Seconds $Seconds; $b = Snap
"rpcs3 pid=$($p.Id) responding=$($p.Responding) title='$($p.MainWindowTitle)' threads=$($b.Count)"
$rows = foreach ($k in $b.Keys) { $d = $b[$k] - $(if ($a.ContainsKey($k)) { $a[$k] } else { 0 })
  [pscustomobject]@{ Tid = $k; CpuPct = [math]::Round(100 * $d / ($Seconds * 1000), 1); Name = [TN]::Name([uint32]$k) } }
$busy = ($rows | Measure-Object CpuPct -Sum).Sum
"total busy = $([math]::Round($busy/100,2)) cores over ${Seconds}s"
$rows | Sort-Object CpuPct -Descending | Select-Object -First $Top | Format-Table -AutoSize | Out-String -Width 200
if ($Dump) {
  "writing minidump to $Dump (process pauses briefly)..."
  $mode = if ($Full) { "full" } else { "" }
  rundll32.exe C:\Windows\System32\comsvcs.dll, MiniDump $p.Id $Dump $mode
  Start-Sleep -Seconds 3
  if (Test-Path $Dump) { "dump: $Dump ($([math]::Round((Get-Item $Dump).Length/1MB)) MB)" } else { "dump FAILED (needs same-user rights)" }
}
