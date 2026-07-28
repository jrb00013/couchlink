# couchlink

**HD, low-latency co-play for emulators.** You host PCSX2 / RPCS3 (or any game window).
Your friend streams your game screen over WebRTC and plays with their own DualSense —
on your machine it shows up as a **Bluetooth DualSense** (`BUS_BLUETOOTH`, Sony VID/PID),
so emulators bind it like a real pad.

Built with the same session / signaling / WebRTC methodologies as [Rohomieo](https://github.com/jrb00013/rohomieo),
and DualSense HID report layouts from [dualsensekit](https://github.com/jrb00013/dualsensekit).

## Why

| Piece | How |
|-------|-----|
| Video | WebRTC + H.264, adaptive FPS (Rohomieo-style GCC + motion idle) |
| Transport | Peer-to-peer media; signaling only for SDP/ICE |
| Path | WireGuard LAN recommended (no public STUN/TURN required) |
| Pad | Custom binary `CLPD` frames on DataChannel `pad` (~250 Hz) |
| Host injection | Linux `uinput` device: name/VID/PID of DualSense, bus = Bluetooth |
| Local capture | hidapi / dualsensekit-compatible USB (`0x01`) or BT (`0x31`) reports |

## Quick start

```bash
./install.sh
source .env.couchlink
couchlink-signaling &
couchlink-host --session-id demo --pin 123456 --preset 1080p60
# friend:
couchlink-client --signaling wss://YOU:8443 --session-id demo --pin 123456
```

See [docs/GETTING_STARTED.md](docs/GETTING_STARTED.md).

## License

MIT
