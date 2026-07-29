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
| Path | **Automatic** — public STUN + local TURN relay (`scripts/start-turn.sh`), router ports opened via UPnP automatically; no VPN or manual router config; WireGuard optional for private LAN-style posture |
| Pad wire format | Custom binary **`CLPD`** on DataChannel `pad` (~rAF / 250 Hz native) |
| Local pad capture | Browser Gamepad API, or Linux hidraw (dualsensekit layouts) |
| Host injection | Linux `uinput` DualSense identity, bus = Bluetooth |
| Invite | Host prints join URL + QR (Rohomieo-style) |

## Install

```bash
git clone https://github.com/jrb00013/couchlink.git
cd couchlink
./install.sh
source .env.couchlink
```

## Run

One command, one terminal — `run.sh` starts signaling + TURN + host as
background child processes, auto-generates a session if you don't have one,
and tears everything down together on Ctrl-C. Detects Linux / WSL / macOS.

```bash
./scripts/run.sh host --local    # same Wi‑Fi (default) — LAN join URL, no UPnP/TURN
./scripts/run.sh host --online   # internet — public IP + TURN + UPnP
# prints the friend's join URL + QR (session, PIN, and TURN creds baked in when online)
```

Friend — open the printed URL. For `--online`, no VPN needed if UPnP (or manual
port forward of **8443/tcp** + **3478/udp+tcp**) works. Press a button on their
DualSense, then Join. Or, for a native client instead of the browser:

```bash
./scripts/run.sh client --online   # Linux / WSL / macOS (needs host join URL / TURN)
./scripts/run.sh client            # same LAN
.\scripts\run.ps1 client           # native Windows (PowerShell)
```

For `--online` as a client, the app prompts for the host’s join URL if unset
(or set `COUCHLINK_JOIN_URL` in `.env.couchlink`). WSL auto-handles ICE host IPs.

The client opens a window showing the host's stream. Plug in a DualSense, or
just use the keyboard (WASD + arrows + Space/Shift/Ctrl/E/Q/R/1/2/Enter/Tab —
see `crates/client/src/keyboard_input.rs` for the full mapping).

**Desktop installers:** [FRIEND_INSTALL.md](docs/FRIEND_INSTALL.md) · [NO_COMPUTER_UX.md](docs/NO_COMPUTER_UX.md) · [DESKTOP_CLIENT.md](docs/DESKTOP_CLIENT.md) · [PLAY_TOGETHER.md](docs/PLAY_TOGETHER.md)

Host role needs Linux `uinput` for the virtual DualSense — run it from Linux
or WSL; macOS/native Windows can only run the friend/client role.

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
- [WireGuard](docs/WIREGUARD.md)
- [Latency](docs/LATENCY.md)
- [Emulators](docs/EMULATORS.md)

## Related

- [rohomieo](https://github.com/jrb00013/rohomieo)
- [dualsensekit](https://github.com/jrb00013/dualsensekit)

## License

MIT
