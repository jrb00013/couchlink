# couchlink

**Full co-play device for emulator nights.** You run PCSX2 or RPCS3. Your friend gets an
HD, low-latency stream of your game and plays with **their own DualSense / PC**. On your
machine their controller shows up as a real **Bluetooth DualSense** (`BUS_BLUETOOTH`,
Sony `054c:0ce6`) — emulators bind it like any wireless pad.

## Stack

| Layer | Implementation |
|-------|----------------|
| Session / PIN / ICE relay | Rohomieo-style WebSocket signaling |
| Video | WebRTC + OpenH264, presets up to **1080p60** |
| Congestion / idle | WebRTC GCC + tile motion detector |
| Path | **WireGuard** recommended (or LAN); no public STUN/TURN by default |
| Pad wire format | Custom binary **`CLPD`** on DataChannel `pad` (~250 Hz) |
| Local pad capture | hidapi + dualsensekit report layouts (`0x01` / `0x31`) |
| Host injection | Linux `uinput` DualSense identity, bus = Bluetooth |

## Install

```bash
git clone https://github.com/jrb00013/couchlink.git
cd couchlink
./install.sh
source .env.couchlink
```

## Run

```bash
# terminal 1
./scripts/start-signaling.sh

# terminal 2 (your PC — emulator host)
./scripts/gen_session.sh   # copy into .env.couchlink
./scripts/start-host.sh

# friend
COUCHLINK_SIGNALING=ws://YOUR_WG_IP:8443/ws ./scripts/start-client.sh
```

Then bind Player 2 in RPCS3/PCSX2 to **DualSense Wireless Controller**.

## Docs

- [Architecture](docs/ARCHITECTURE.md)
- [Protocol](docs/PROTOCOL.md) (signaling + CLPD)
- [Getting started](docs/GETTING_STARTED.md)
- [WireGuard](docs/WIREGUARD.md)
- [Latency](docs/LATENCY.md)
- [Emulators](docs/EMULATORS.md)

## Related

- [rohomieo](https://github.com/jrb00013/rohomieo) — remote desktop methodologies
- [dualsensekit](https://github.com/jrb00013/dualsensekit) — DualSense HID / RPCS3 pad binding

## License

MIT
