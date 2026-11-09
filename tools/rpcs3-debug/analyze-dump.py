#!/usr/bin/env python3
"""Per-thread instruction pointers from a small RPCS3 minidump (+ thread names, + PDB symbols).

  analyze-dump.py rpcs3-hang.dmp [rpcs3.pdb]

Needs the 'minidump' package (python3 -m venv ~/.venvs/md && ~/.venvs/md/bin/pip install minidump).
Thread names come from <dmp>.names.csv (written by threads.ps1 -Dump). Threads parked in ntdll/win32u
are waiting; threads whose RIP is in rpcs3.exe are executing emulator code (interpreter, HLE, RSX...);
RIP in no module = JIT code (LLVM-recompiled PPU/SPU). Offsets resolve through symbolize.py if a PDB is given.
"""
import os, subprocess, sys
from minidump.minidumpfile import MinidumpFile

dmp = sys.argv[1]; pdb = sys.argv[2] if len(sys.argv) > 2 else None
m = MinidumpFile.parse(dmp)
mods = sorted(((x.baseaddress, x.size, x.name.split('\\')[-1]) for x in m.modules.modules))
names = {}
nf = dmp + ".names.csv"
if os.path.exists(nf):
    for line in open(nf, encoding="utf-8-sig"):
        tid, _, n = line.rstrip("\n").partition(",")
        if tid.strip().isdigit(): names[int(tid)] = n.strip()

def where(a):
    for b, s, n in mods:
        if b <= a < b + s: return n, a - b
    return None, None

rows = []
for t in m.threads.threads:
    rip = getattr(getattr(t, 'ContextObject', None), 'Rip', None)
    n, off = where(rip) if rip else (None, None)
    rows.append((t.ThreadId, names.get(t.ThreadId, "?"), rip, n, off))

def sym(offs):
    if not pdb or not offs: return {}
    here = os.path.dirname(os.path.abspath(__file__))
    out = subprocess.run([sys.executable, os.path.join(here, "symbolize.py"), pdb] + [hex(o) for o in offs],
                         capture_output=True, text=True).stdout.splitlines()
    return dict(zip(offs, out))

exe_offs = [r[4] for r in rows if r[3] and r[3].lower().startswith("rpcs3")]
sy = sym(exe_offs)
print(f"{len(rows)} threads")
print("EXECUTING in rpcs3.exe:")
for tid, name, rip, n, off in rows:
    if n and n.lower().startswith("rpcs3"):
        print(f"  {tid:>6} {name:<40} rpcs3+0x{off:x}  {sy.get(off,'')}")
print("EXECUTING in JIT/anonymous code:")
for tid, name, rip, n, off in rows:
    if rip and not n: print(f"  {tid:>6} {name:<40} rip=0x{rip:x}")
print("WAITING (ntdll/win32u):")
for tid, name, rip, n, off in rows:
    if n and n.lower() in ("ntdll.dll", "win32u.dll", "kernelbase.dll"):
        print(f"  {tid:>6} {name:<40} {n}+0x{off:x}")
