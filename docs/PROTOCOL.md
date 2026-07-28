# Couchlink wire protocol

## Signaling (WebSocket `/ws`)

Rohomieo-style tagged JSON (`type` in snake_case). Media never transits this server.

| Message | Direction | Fields |
|---------|-----------|--------|
| `register_host` | host → server | `session_id`, `pin`, `device_name?`, `preset?`, `emulator?` |
| `register_player` | player → server | `session_id`, `pin`, `player_name?` |
| `registered` | server → client | `role`, `session_id` |
| `offer` / `answer` | relayed | `sdp`; `offer` includes monotonic `epoch` |
| `ice_candidate` | relayed | `candidate`, `sdpMid?`, `sdpMLineIndex?` |
| `request_offer` | player → host (relayed) | — (renegotiate; host does not rebuild peer) |
| `stream_info` | host → player | `width`, `height`, `fps`, `codec` |
| `heartbeat` / `pong` | either | — |
| `peer_joined` / `peer_left` | server | `role`; `peer_joined` includes `epoch` |

## DataChannel `pad` — custom binary `CLPD`

Fixed-size little-endian frame (lower latency than JSON at ~250 Hz):

| Offset | Len | Field |
|--------|-----|-------|
| 0 | 4 | Magic `CLPD` |
| 4 | 1 | Version `1` |
| 5 | 4 | `seq` |
| 9 | 4 | `buttons` bitfield |
| 13 | 1 | `lx` |
| 14 | 1 | `ly` |
| 15 | 1 | `rx` |
| 16 | 1 | `ry` |
| 17 | 1 | `l2` |
| 18 | 1 | `r2` |
| 19 | 2 | `gx` |
| 21 | 2 | `gy` |
| 23 | 2 | `gz` |
| 25 | 1 | `touch_active` |
| 26 | 2 | `touch_x` |
| 28 | 2 | `touch_y` |
| 30 | 1 | reserved |

Button bits mirror DualSense face/shoulder/dpad layout used by dualsensekit parsers.

## Pad feedback (host → player, JSON on `pad` channel)

```json
{"type":"rumble","large":120,"small":40}
{"type":"lightbar","r":0,"g":0,"b":255}
{"type":"player_led","mask":1}
```

## DualSense HID (client capture)

From dualsensekit `PROTOCOL.md`:

| Report | ID | Notes |
|--------|----|-------|
| USB input | `0x01` | 64 bytes |
| BT input | `0x31` | 78 bytes |
| USB output | `0x02` | rumble / lightbar |

## HTTP

| Path | Purpose |
|------|---------|
| `GET /health` | Liveness |
| `GET /api/status` | Version + session counts |
| `GET /api/audit` | PIN / join audit |
| `GET /metrics` | Prometheus |
