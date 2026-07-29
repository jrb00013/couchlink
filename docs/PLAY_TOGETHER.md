# Playing together (host + friend across the world)

One host runs the game. One friend joins from **anywhere** with the browser or the **Couchlink Player** desktop app (Windows / Linux / macOS).

> **WSL + Windows games (PCSX2/RPCS3):** `./install.sh` and host start **auto-build** `couchlink-win-capture.exe` via Windows cargo. Host start opens the **Windows capture picker** so you choose which window/monitor to stream (or set `COUCHLINK_CAPTURE_SOURCE=desktop` / `COUCHLINK_CAPTURE_WINDOW=PCSX2`). WSL listens on TCP **9876**; Windows connects outbound via localhost forwarding.

Internet play uses **STUN + your host’s TURN relay** (started automatically with `./scripts/run.sh host`) and **UPnP** when your router supports it.

---

## Part 1 — You (host)

### 1. One-time setup (gaming PC)

```bash
git clone https://github.com/jrb00013/couchlink.git
cd couchlink
git checkout worktree-native-video-viewer   # until merged to main
./install.sh
source .env.couchlink
```

- Use **native Linux** or **WSL2** with uinput working (see `install.sh` / README).
- Install **PCSX2** or **RPCS3** and your game as usual.

### 2. Start a session

```bash
./scripts/run.sh host --local     # couch / same Wi‑Fi
./scripts/run.sh host --online    # friend over the internet
```

`--local` (default) prints a LAN join URL and skips UPnP/TURN.  
`--online` fetches your public IP, starts TURN, and opens ports via UPnP.

This starts, in one terminal:

- Signaling + web UI on **`:8443`**
- **TURN** on **`:3478`** (online only)
- **couchlink-host** (capture + encode + virtual DualSense for player 2)

Watch the log for:

```text
friend join URL: http://…/?s=…&p=…&auto=1&ws=ws://…&turn=turn:…&turnu=…&turnp=…
```

Copy that **entire URL** (or QR) and send it to your friend (Discord, Signal, etc.). That link already includes session, PIN, signaling WebSocket, and TURN credentials (when online).

### 3. Make yourself reachable from the internet (`--online`)

The join URL must use addresses your friend can reach—not `127.0.0.1`.

1. **Router:** Ensure **TCP 8443** (signaling/web) and **UDP+TCP 3478** (TURN) reach your PC. `run.sh host --online` tries **UPnP** automatically; if it fails, forward those ports manually to your gaming machine’s LAN IP.
2. **Public IP:** Detected via `ifconfig.me`. If that fails or you’re on CGNAT, set in `.env.couchlink` before starting:
   - `COUCHLINK_PUBLIC_IP=your.public.ip`
3. **Firewall on the host OS:** Allow inbound **8443/tcp** and **3478/udp+tcp**.

If the printed join URL still shows a bad address, set `COUCHLINK_PUBLIC_IP`, restart with `--online`, and send the **new** join URL.

### 4. Emulator

- Start your game on **Player 1** (your physical pad).
- Bind **Player 2** to **DualSense Wireless Controller** — the **virtual Bluetooth pad** couchlink creates when your friend connects and sends input.

---

## Part 2 — Your friend (any country)

Pick **browser** (zero install) or **desktop player** (native app).

### Option A — Browser (simplest)

1. Open the **join URL** you sent (Chrome/Edge/Firefox).
2. Press any button on a **DualSense** (or compatible pad) so the browser unlocks Gamepad API.
3. Click **Join** (or it auto-joins if `auto=1`).
4. Full-screen the video; play.

### Option B — Desktop player (Windows / Linux / macOS)

1. Get the build from you (or build from repo — see [DESKTOP_CLIENT.md](DESKTOP_CLIENT.md)):
   - **Windows:** unzip → run `install-client.ps1` → paste join URL when asked.
   - **Linux:** run the **AppImage**; set `join_url=` in `~/.config/couchlink/config`.
   - **macOS:** open **Couchlink Player.app**; same config path under `~/Library/Application Support/Couchlink/config`.
2. Double-click **Couchlink Player** (or run the AppImage).
3. Plug in **DualSense**, or use **keyboard** (WASD, arrows, Space/Shift/Ctrl/E, etc.).

No second VPN required if TURN + port forwarding (or UPnP) work.

---

## Part 3 — During the session

| Step | Host | Friend |
|------|------|--------|
| Video | Runs game; host encodes screen | Sees stream in browser or native window |
| Input | Player 1 local pad | Player 2 via virtual DualSense on host |
| Quit | Ctrl-C in `run.sh host` terminal | Close window / Esc, or leave browser tab |
| Reconnect | Same join URL while host still running | Open link or launcher again |

---

## Troubleshooting (long distance)

| Symptom | Likely fix |
|---------|------------|
| Friend connects but **no video** | Check host firewall; confirm host log shows “player joined” and stream started |
| **Never connects** | Join URL must use **public** IP/hostname; verify **8443** and **3478** forwarded |
| **High latency** | Expected over internet; lower host preset in `.env` (`COUCHLINK_PRESET=720p60`) |
| Native client **no window** | GPU/driver issue → falls back to headless; friend uses browser instead |
| **Pad not in emulator** | RPCS3/PCSX2 Player 2 bound to virtual DualSense; friend must send input (pad or keyboard) |

---

## Minimal checklist

1. Host: `./scripts/run.sh host --online` (or `--local` on the same Wi‑Fi).  
2. Host: send **full join URL** from the log/QR.  
3. Friend: open URL **or** install desktop player + `join_url` config.  
4. Friend: pad or keyboard.  
5. Host: Player 2 = virtual DualSense in emulator.  
6. Play.
