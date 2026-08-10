# Couchlink architecture

High-definition, low-latency **co-play** for emulators: host streams the game window;
the friend's DualSense is injected on the host as a **Bluetooth DualSense**.

Methodologies follow [Rohomieo](https://github.com/jrb00013/rohomieo): WebRTC media is
peer-to-peer; signaling only exchanges SDP/ICE; WireGuard LAN preferred (no public
STUN/TURN). Pad HID layouts follow [dualsensekit](https://github.com/jrb00013/dualsensekit).

## Components

| Crate / dir | Role |
|-------------|------|
| `crates/proto` | JSON signaling + binary `CLPD` pad frames |
| `crates/pad` | DualSense / DS4 / Xbox parse + Linux `uinput` / Windows VHID+ViGEm virtual pad |
| `crates/signaling` | Axum WebSocket session broker |
| `crates/host` | Capture → H.264 → WebRTC; apply pad frames to virtual device |
| `crates/client` | hidraw DualSense/DS4/Xbox reader + WebRTC answer + pad sender |
| `web/` | React player — WebRTC video + Gamepad API → CLPD |
| `infra/wireguard` | VPN examples for friend↔you path |
| `adapters/` | PCSX2 / RPCS3 binding helpers |

## Connection flow

1. Host registers `register_host` with `session_id` + `pin` + stream preset.
2. Player registers `register_player` with same credentials.
3. Server sends `peer_joined` to host.
4. Host creates WebRTC **offer** (H.264 video + `pad` data channel).
5. Player **answer** + ICE via signaling relay.
6. Encrypted SRTP video + binary pad frames flow peer-to-peer (ideally over WireGuard).

## Virtual pad

**Linux:** the host opens `/dev/uinput` and creates a device with:

- `BUS_BLUETOOTH` (0x05)
- Vendor `0x054C`, Product `0x0CE6` (DualSense)
- Name `DualSense Wireless Controller`

**Windows:** Auto prefers a custom DualSense VHID pipe (`\\.\pipe\couchlink-ds-vhid`),
then falls back to Nefarius [ViGEmBus](https://github.com/nefarius/ViGEmBus/releases)
DualShock 4, then Xbox 360. Override with `COUCHLINK_VIRTUAL_PAD=auto|dualsense|ds4|xbox360|noop`.

PCSX2 / RPCS3 see a wireless DualSense (Linux) or ViGEm DS4/Xbox (Windows) and can
bind player 2 to it — same outcome dualsensekit's `rpcs3_configure_pads.ps1` targets
for local pads.

The client reads whatever physical pad is plugged in — DualSense, DualShock 4, or an
Xbox One/Series controller, both over `hidraw` — and normalizes it onto the same
`PadFrame` wire format. On Linux the virtual pad is always DualSense identity;
on Windows Auto may present DualSense VHID, DS4, or Xbox 360 depending on drivers.
Xbox face buttons are remapped by position (A→bottom, B→right, X→left, Y→top) to land
correctly on the DualSense diamond.

## Adaptive streaming

- WebRTC GCC on the video track
- Tile-diff motion detector: idle ~8 FPS when &lt;2% tiles change; motion up to preset FPS
- Presets: `1080p60`, `1080p30`, `720p60`, `720p30`
