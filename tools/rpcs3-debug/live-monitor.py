#!/usr/bin/env python3
"""Live monitor for an RPCS3 + CouchLink session. Prints one line per EVENT (not per sample) so it can be used
as a Claude Code Monitor, and appends a full time series to ~/rpcs3-live/<session>.csv for later analysis.

  tools/rpcs3-debug/live-monitor.py [--interval 5]

Signals (all read-only, none pause the game):
  fps        window-title FPS (powershell Get-Process)             event: fps drops < 30, recovers
  silence    seconds since the newest non-noise game log line       event: stall start (>= 15 s) / stall end
  capture    CouchLink capture bytes/s on :9876 (ss -ti)            event: capture stops flowing / resumes
  contention 'PPU reservation contention' log lines (every 10 s     event: each line (failures, SPU back-offs,
             when >= 20k stwcx. failures)                           starvation refreshes, per-cause counts)
  game       RPCS3 gone / fatal error / Fatal Error dialog          event: immediately
Needs a fork build >= 40500ef for the contention lines; >= 4f2b34e for refresh counts.
"""
import argparse, csv, os, re, subprocess, sys, time

LOG = os.environ.get("RPCS3_LOG", "/mnt/c/Users/josep/RPCS3/log/RPCS3.log")
NOISE = re.compile(r"cellAudio|cellMic|sys_event|sys_mmapper|DualSense|Performance|Syscall Usage|PERF:|"
                   r"RSX: (Add program|Program compiled)|SPU: (Building|New SPU block)|reservation contention")
LINE = re.compile(r"^·[A-Z!] (\d+):(\d+):([\d.]+) ")


def secs(h, m, s):
    return int(h) * 3600 + int(m) * 60 + float(s)


def tail(path, nbytes):
    try:
        with open(path, "rb") as f:
            f.seek(0, 2)
            n = f.tell()
            f.seek(max(0, n - nbytes))
            return n, f.read().decode("utf-8", "replace").replace("\r", "")
    except OSError:
        return 0, ""


def ps(cmd):
    try:
        return subprocess.run(["powershell.exe", "-NoProfile", "-Command", cmd], capture_output=True, text=True, timeout=20).stdout.replace("\r", "").strip()
    except Exception:
        return ""


def capture_rx():
    try:
        out = subprocess.run(["ss", "-ti", "state", "established", "( sport = :9876 )"], capture_output=True, text=True, timeout=5).stdout
        m = re.search(r"bytes_received:(\d+)", out)
        return int(m.group(1)) if m else None
    except Exception:
        return None


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--interval", type=float, default=5)
    args = ap.parse_args()
    os.makedirs(os.path.expanduser("~/rpcs3-live"), exist_ok=True)
    csv_path = os.path.expanduser("~/rpcs3-live/%s.csv" % time.strftime("%Y%m%d-%H%M%S"))
    cf = open(csv_path, "w", newline="")
    w = csv.writer(cf)
    w.writerow(["wall", "emu_time", "fps", "silence_s", "capture_KBps", "stcx_fail_10s", "backoffs", "refreshes"])
    print("monitor started; series -> %s" % csv_path, flush=True)

    state = {"fps_low": False, "stall": False, "cap_dead": False, "gone": False}
    last_rx = None
    last_t = time.time()
    seen_contention = set()
    fps = None
    last_ps = 0.0
    stall_t0 = None
    last = {"stcx": "", "back": "", "ref": ""}

    while True:
        time.sleep(args.interval)
        now = time.time()
        wall = time.strftime("%H:%M:%S")

        # ---- title / process (powershell is slowish, every 10 s)
        if now - last_ps >= 10:
            last_ps = now
            t = ps("(Get-Process rpcs3 -EA SilentlyContinue | % { $_.MainWindowTitle }) -join '|'")
            if not t:
                if not state["gone"]:
                    state["gone"] = True
                    print("%s GAME: rpcs3.exe is not running" % wall, flush=True)
            else:
                if state["gone"]:
                    state["gone"] = False
                    print("%s GAME: rpcs3.exe is back" % wall, flush=True)
                if "Fatal Error" in t:
                    print("%s GAME: 'RPCS3: Fatal Error' dialog is open" % wall, flush=True)
                m = re.search(r"FPS: ([\d.]+)", t)
                fps = float(m.group(1)) if m else None
                low = fps is not None and fps < 30
                if low and not state["fps_low"]:
                    print("%s FPS dropped to %.1f" % (wall, fps), flush=True)
                if state["fps_low"] and fps is not None and not low:
                    print("%s FPS recovered: %.1f" % (wall, fps), flush=True)
                state["fps_low"] = low

        # ---- log: silence, emu time, contention lines, fatal
        size, txt = tail(LOG, 3_000_000)
        emu = None
        last_any = last_game = None
        for l in txt.split("\n"):
            m = LINE.match(l)
            if not m:
                continue
            s = secs(*m.groups())
            last_any = s
            emu = "%s:%s:%s" % (m.group(1), m.group(2), m.group(3)[:5])
            if not NOISE.search(l):
                last_game = s
            if "reservation contention" in l:
                key = l[:40]
                if key not in seen_contention:
                    seen_contention.add(key)
                    f = re.search(r"failures since last report: (\d+)", l)
                    b = re.search(r"back-offs for PPU priority, total: (\d+)", l)
                    r = re.search(r"starvation refreshes=(\d+)", l)
                    last["stcx"], last["back"], last["ref"] = (f.group(1) if f else ""), (b.group(1) if b else ""), (r.group(1) if r else "")
                    causes = re.search(r"cumulative by cause: ([^)]*)\)", l)
                    # Quiet by default: one line per minute, plus whenever the refresh count changes or during a stall.
                    changed = last["ref"] != last.get("ref_printed")
                    if (changed and not state["stall"] and int(last["ref"] or 0) - int(last.get("ref_n", 0) or 0) >= 50) or now - last.get("t_printed", 0) >= 120:
                        last["ref_n"] = last["ref"]
                        last["ref_printed"], last["t_printed"] = last["ref"], now
                        print("%s CONTENTION: %s stcx failures/10s, SPU back-offs=%s, refreshes=%s | %s" % (
                            wall, last["stcx"], last["back"] or "?", last["ref"] or "n/a", causes.group(1) if causes else "no cause split (older build)"), flush=True)
            if l.startswith("·F "):
                k = "F" + l[:60]
                if k not in seen_contention:
                    seen_contention.add(k)
                    print("%s FATAL: %s" % (wall, l[:200]), flush=True)
        silence = int(last_any - last_game) if (last_any is not None and last_game is not None) else 0
        if silence >= 15 and not state["stall"]:
            state["stall"] = True
            stall_t0 = now
            print("%s STALL START: no game event for %ds (emu %s, fps %s)  -> run: tools/rpcs3-debug/rpcs3-hang-diag.sh --guest --shot" % (wall, silence, emu, fps), flush=True)
        elif state["stall"] and silence < 5:
            state["stall"] = False
            print("%s STALL END after ~%ds" % (wall, int(now - (stall_t0 or now))), flush=True)

        # ---- capture throughput
        rx = capture_rx()
        kbps = None
        if rx is not None and last_rx is not None and now > last_t:
            kbps = (rx - last_rx) / (now - last_t) / 1024
        last_rx, last_t = rx, now
        dead = (rx is None) or (kbps is not None and kbps < 50)
        if dead and not state["cap_dead"]:
            print("%s CAPTURE: no data from win-capture (%s KB/s) - friends see a frozen/black picture" % (wall, "not connected" if rx is None else "%.0f" % kbps), flush=True)
        if state["cap_dead"] and not dead:
            print("%s CAPTURE: flowing again (%.0f KB/s)" % (wall, kbps or 0), flush=True)
        state["cap_dead"] = dead

        w.writerow([wall, emu or "", fps if fps is not None else "", silence, "%.0f" % kbps if kbps is not None else "", last["stcx"], last["back"], last["ref"]])
        cf.flush()


if __name__ == "__main__":
    try:
        main()
    except KeyboardInterrupt:
        pass
