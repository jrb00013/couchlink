# couchlink

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

```bash
# terminal 1 — signaling + player web UI
./scripts/start-signaling.sh

# terminal 2 — local TURN relay (safe to always leave running).
# Auto-opens the needed router port via UPnP — no router login needed
# (falls back to a warning if your router doesn't support UPnP).
./scripts/start-turn.sh

# terminal 3 — your PC (emulator host)
./scripts/gen_session.sh   # paste into .env.couchlink
./scripts/start-host.sh    # prints join URL + QR (TURN creds baked in automatically)

# friend — open the printed URL (works from anywhere, no VPN setup)
# press a button on DualSense, then Join
```

Native client alternative (hidraw pad sender):

```bash
COUCHLINK_SIGNALING=ws://YOUR_PUBLIC_HOST:8443/ws ./scripts/start-client.sh
```

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
