# Per-thread CPU deltas (with thread names) and optional full minidump for a live rpcs3.exe.
# Run on WINDOWS:  powershell -NoProfile -ExecutionPolicy Bypass -File threads.ps1 [-Seconds 5] [-Dump C:\path\rpcs3.dmp]
# Non-invasive: reads counters only; -Dump uses comsvcs.dll MiniDump (pauses the process for a few seconds).
param([int]$Seconds = 5, [string]$Dump = "", [int]$Top = 25)
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
  rundll32.exe C:\Windows\System32\comsvcs.dll, MiniDump $p.Id $Dump full
  Start-Sleep -Seconds 3
  if (Test-Path $Dump) { "dump: $Dump ($([math]::Round((Get-Item $Dump).Length/1MB)) MB)" } else { "dump FAILED (needs same-user rights)" }
}
