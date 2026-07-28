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
