# couchlink
<img width="1918" height="898" alt="image" src="https://github.com/user-attachments/assets/5f855a69-77e1-4d20-8bf6-b5a90e00fcae" />


**Full co-play device for emulator nights.** You run PCSX2 or RPCS3. Your friend opens a
browser (or native client), gets an **HD low-latency** stream of your game, and plays with
**their own DualSense**. On your machine that pad shows up as a real **Bluetooth DualSense**
(`BUS_BLUETOOTH`, Sony `054c:0ce6`) — emulators bind it like any wireless controller.

## Stack

| Layer | Implementation |
|-------|----------------|
| Friend UI | React player at `:8443` — WebRTC video + Gamepad API → `CLPD` |
| Session / PIN / ICE | Rohomieo-style WebSocket signaling |
| Video | WebRTC + OpenH264, presets up to **1080p60**, scaled capture |
| Congestion / idle | WebRTC GCC + tile motion detector |
| Path | **PRIME mesh** — **Headscale** (default, no Tailscale Inc account), else Tailscale / WireGuard; else public STUN + local TURN; on WSL `--online` also firewall + WSL portproxy via **Couchlink Helper**; then HTTPS (cloudflared) + IPv6 TURN if UPnP is off |
| Pad wire format | Custom binary **`CLPD`** on DataChannel `pad` (~rAF / 250 Hz native) |
| Local pad capture | Browser Gamepad API, or Linux hidraw (DualSense / DS4 / Xbox) |
| Host injection | Linux `uinput` DualSense BT; Windows DualSense VHID → ViGEm DS4 → Xbox 360 |
| Invite | Host prints join URL + QR (`mesh=headscale&hs=&tskey=` when on Headscale) |

## Install

```bash
git clone https://github.com/jrb00013/couchlink.git
cd couchlink

# Friend (default): player deps — paste the host join URL (Headscale auto-joins from the link)
./install.sh
./install.sh --online

# Host (gaming PC): full stack + Headscale mesh
./install.sh --host --online
source .env.couchlink
```

**WSL / Windows host (one-time):** install **Couchlink Helper** so later `--online` runs need **no UAC**:

```bash
./scripts/install-windows-helper.sh
# or: packaging/windows/build-helper-installer.ps1 → CouchlinkHelper-Setup.exe
```

See [NO_COMPUTER_UX.md](docs/NO_COMPUTER_UX.md) · [HEADSCALE.md](docs/HEADSCALE.md).

## Run

One command, one terminal — `run.sh` starts signaling + TURN + host as
background child processes, auto-generates a session if you don't have one,
and tears everything down together on Ctrl-C. Detects Linux / WSL / macOS.

```bash
./scripts/run.sh host --local    # same Wi‑Fi (default) — LAN join URL, no UPnP/TURN
./scripts/run.sh host --online   # internet — Headscale/Tailscale/WireGuard if up, else TURN + UPnP / Cloudflare
# Mesh: Headscale is PRIME (./scripts/enable-headscale.sh). Optional: Tailscale cloud / WireGuard — docs/MESH.md
# WSL: install Couchlink Helper once, then --online does firewall/portproxy with no UAC
# prints the friend's join URL + QR (session, PIN, hs/tskey, and TURN creds when needed)
```

Friend — open the printed URL. On **Headscale**, `./install.sh --online` + paste the link
auto-joins (no Tailscale Inc login). On Tailscale cloud / WireGuard, they must be on the
same mesh. Otherwise, for public `--online`, no VPN needed if UPnP (or manual port forward
of **8443/tcp** + **3478/udp+tcp**) works. Press a button on their DualSense, then Join.
Or, for a native client instead of the browser:

```bash
./scripts/run.sh client --online   # Linux / WSL / macOS (needs host join URL / TURN)
./scripts/run.sh client            # same LAN
.\scripts\run.ps1 client           # native Windows (PowerShell)
```

For `--online` as a client, the app prompts for the host’s join URL if unset
(or set `COUCHLINK_JOIN_URL` in `.env.couchlink`). WSL auto-handles ICE host IPs.
Optional: `./install.sh --online --unblock-firewall`.

The client opens a window showing the host's stream. Plug in a DualSense, or
just use the keyboard (WASD + arrows + Space/Shift/Ctrl/E/Q/R/1/2/Enter/Tab —
see `crates/client/src/keyboard_input.rs` for the full mapping).

**Desktop installers:** [FRIEND_INSTALL.md](docs/FRIEND_INSTALL.md) · [NO_COMPUTER_UX.md](docs/NO_COMPUTER_UX.md) · [DESKTOP_CLIENT.md](docs/DESKTOP_CLIENT.md) · [PLAY_TOGETHER.md](docs/PLAY_TOGETHER.md)

Host role needs a virtual pad: Linux/`uinput` (DualSense), or native **Windows** with
[ViGEmBus](https://github.com/nefarius/ViGEmBus/releases) (and optional DualSense VHID
companion). Override with `COUCHLINK_VIRTUAL_PAD=auto|dualsense|ds4|xbox360|noop`.
macOS auto-detects and can run **client**, signaling, and a **video-only host** (no pad injection).

<details>
<summary>Individual scripts (if you want separate terminals/logs)</summary>

```bash
./scripts/start-signaling.sh   # signaling + player web UI
./scripts/start-turn.sh        # local TURN relay, auto UPnP port
./scripts/gen_session.sh       # paste output into .env.couchlink
./scripts/start-host.sh
./scripts/start-client.sh
```
</details>

Bind Player 2 in RPCS3/PCSX2 to **DualSense Wireless Controller**.

## Docs

- [Architecture](docs/ARCHITECTURE.md)
- [Protocol](docs/PROTOCOL.md)
- [Getting started](docs/GETTING_STARTED.md)
- [Playing together](docs/PLAY_TOGETHER.md)
- [Headscale mesh](docs/HEADSCALE.md) — **PRIME** path (no Tailscale Inc account)
- [Mesh overview](docs/MESH.md) — Headscale / Tailscale / WireGuard
- [WireGuard](docs/WIREGUARD.md)
- [No-computer UX](docs/NO_COMPUTER_UX.md) — installers + Couchlink Helper
- [Latency](docs/LATENCY.md)
- [Emulators](docs/EMULATORS.md)

## Related

- [rohomieo](https://github.com/jrb00013/rohomieo)
- [dualsensekit](https://github.com/jrb00013/dualsensekit)

## License

MIT
