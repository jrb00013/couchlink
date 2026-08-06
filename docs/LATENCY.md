# Latency budget (HD co-play)

Target feel for local/WireGuard play: **&lt; 40–60 ms** glass-to-glass on a good LAN.

| Stage | Budget | Notes |
|-------|--------|-------|
| Capture | 0–8 ms | scrap DXGI/X11; prefer exclusive fullscreen / borderless |
| Encode | 4–12 ms | OpenH264 low-latency; 1080p60 needs CPU headroom |
| Net | 5–30 ms | WireGuard preferred; Wi-Fi adds jitter |
| Decode + present | 8–16 ms | Player GPU decode |
| Pad | ~4 ms | 250 Hz `CLPD` frames; uinput inject on host |

## Knobs

- `--preset 720p60` if 1080p60 saturates encode
- `--idle-fps 8` (default) saves bitrate on static menus (Rohomieo motion detector)
- Wired Ethernet + WG over Wi-Fi
- Keep host display Hz ≥ stream FPS

## Custom proto vs WebRTC

Pad path is already custom binary. Replacing video with a proprietary UDP codec is
optional future work; WebRTC GCC already adapts bitrate under congestion.

## Relay reachability under WSL (why ICE can fail for one friend and not another)

WSL2's default NAT mode gives the VM a private IPv4 and **no IPv6 at all**. The
only inbound bridge is `netsh interface portproxy`, which is **TCP-only**, so
coturn — which needs inbound UDP — cannot be reached from outside at all.

The invite still advertised the *Windows* IPv6 for TURN, an address the relay
could never answer on. The failure is silent and asymmetric:

* A friend whose NAT allows hole-punching connects directly and never notices.
* A friend behind a stricter NAT needs the relay, gathers no `typ relay`
  candidate, and ICE fails. It looks exactly like *their* network being broken.

`run.sh` now refuses to advertise an IPv6 TURN address this machine does not
actually hold (`couchlink_owns_ipv6`), and says so. Signaling still uses the
Windows IPv6, which is correct — that is TCP and portproxy forwards it.

To get a working relay, pick one:

| Fix | Cost |
|-----|------|
| `./scripts/enable-wsl-mirrored.sh` then `wsl --shutdown` | WSL shares the Windows addresses; no router involvement. Needs Windows 11 build 22621+ |
| Forward UDP+TCP 3478 to this PC | Router change; works on any Windows build |

Mirrored networking is the durable answer: it removes the NAT layer, so coturn
binds the same global IPv6 the invite advertises.
