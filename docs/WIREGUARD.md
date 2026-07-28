# WireGuard path (recommended)

Same posture as Rohomieo: keep WebRTC on a private mesh. Signaling can listen on the
WireGuard IP; media uses host candidates on that interface. **No public STUN/TURN**
required when both peers are on the VPN.

## Minimal two-peer config

See `infra/wireguard/wg0-host.conf.example` and `wg0-player.conf.example`.

1. Generate keys on each machine (`wg genkey`).
2. Exchange public keys + assign `10.66.0.1/24` (host) and `10.66.0.2/24` (player).
3. `wg-quick up wg0`
4. Point client at `ws://10.66.0.1:8443/ws`

## Why not only a custom UDP proto?

Couchlink **does** use a custom binary pad proto (`CLPD`) on the DataChannel.
Video still rides WebRTC/SRTP for congestion control, encryption, and NAT-friendly
ICE — proven in Rohomieo. You can later add a raw UDP video path; WireGuard + WebRTC
is the low-risk HD default.
