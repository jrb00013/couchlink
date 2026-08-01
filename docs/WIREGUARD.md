# WireGuard path (Tier A mesh)

WireGuard gives couchlink a private `/24` so signaling and media look like LAN
(`http://10.66.0.1:8443/...`) without public TURN or Cloudflare.

**Prefer Tailscale when you can** ([MESH.md](MESH.md)) — less port-forward pain.
Use WireGuard when you want keys/configs entirely under your control.

`./scripts/run.sh host --online` **auto-detects** an up `wg0` and prints a mesh
join URL (PRIME path). If WireGuard is down, it falls through to UPnP →
Cloudflare / IPv6 → bore.

## When to use

| Situation | Path |
|-----------|------|
| Friend can install Tailscale | [MESH.md](MESH.md) Tailscale |
| You want self-hosted keys only | WireGuard (this doc) |
| Zero install for friend | UPnP / port forward / Cloudflare fallback |

## Prerequisites

- `wireguard-tools` (`wg`, `wg-quick`) on Linux/macOS, or **Windows WireGuard** app on WSL hosts  
- **UDP 51820** inbound to the host (router forward or open firewall), **or** a relay/VPS Endpoint  
- Friend can import a `.conf` and keep `PersistentKeepalive = 25` (already in generated player conf)

## Generate configs

```bash
./scripts/setup-wireguard.sh
# or: COUCHLINK_INSTALL_MESH=1 ./install.sh
```

Writes (gitignored):

- `infra/wireguard/keys/{host,player}.{key,pub}`
- `infra/wireguard/wg0-host.conf` — `10.66.0.1/24`, ListenPort `51820`
- `infra/wireguard/wg0-player.conf` — `10.66.0.2/24`, Endpoint `YOUR_PUBLIC_IP:51820`

Idempotent. Rotate keys: `./scripts/setup-wireguard.sh --rotate`.

Templates without secrets: `infra/wireguard/*.example`.

## Host bring-up

### Native Linux

```bash
sudo install -m 600 infra/wireguard/wg0-host.conf /etc/wireguard/wg0.conf
sudo wg-quick up wg0
wg show
./scripts/run.sh host --online
# expect: PRIME mesh (wireguard) — join via http://10.66.0.1:8443/
```

Allow **UDP 51820** on the host firewall and forward it on the router to this machine.

### WSL (recommended layout)

WireGuard **inside** WSL2 is painful. Prefer:

1. Import `wg0-host.conf` into the **Windows WireGuard** app and activate it.  
2. Ensure Windows can reach couchlink in WSL on TCP **8443** (existing WSL portproxy from `--online` prep helps for public paths; for mesh, friend hits `10.66.0.1` on **Windows** — you may need to portproxy `10.66.0.1:8443` → WSL or run with mirrored networking).  
3. If detection can’t see `wg0` from WSL, force the invite IP:

```bash
export COUCHLINK_WG_HOST_IP=10.66.0.1
export COUCHLINK_MESH=wireguard
export COUCHLINK_MESH_IP=10.66.0.1
./scripts/run.sh host --online
```

Spike your exact WSL networking before relying on this in production play nights.

### macOS

`brew install wireguard-tools`, place conf under `/opt/homebrew/etc/wireguard/` (or use the WireGuard App Store client), then `wg-quick up`. macOS host is video-only for pad injection.

## Friend bring-up

1. Receive `wg0-player.conf` out of band (Signal, etc.) — **never commit it**.  
2. Import / `wg-quick up wg0`.  
3. `ping 10.66.0.1`.  
4. Open the host join URL or:

```bash
./scripts/run.sh client --online
# paste http://10.66.0.1:8443/?s=…&p=…&auto=1&ws=ws://10.66.0.1:8443/ws
```

**Native client preferred.** Browser WebCodecs needs a secure context; plain `http://10.66.0.x` falls back to higher-latency paths.

## Manual minimal config (no setup script)

See `infra/wireguard/wg0-host.conf.example` and `wg0-player.conf.example`.

1. `wg genkey` on each side; exchange public keys.  
2. Host `10.66.0.1/24`, player `10.66.0.2/24`.  
3. `wg-quick up wg0`.  
4. Point client at `ws://10.66.0.1:8443/ws`.

## Troubleshooting

| Symptom | Check |
|---------|--------|
| `run.sh --online` still uses Cloudflare | `wg show wg0` — interface down; or `COUCHLINK_SKIP_MESH=1` set |
| Handshake never completes | Endpoint IP/port, UDP 51820 firewall/router, correct peer public key |
| Ping works, join fails | Host listening `0.0.0.0:8443`; friend using `10.66.0.1` not LAN IP |
| WSL: no `wg0` | Use Windows WireGuard + `COUCHLINK_MESH_IP` override |

## Security

- Keys and generated confs are gitignored (`infra/wireguard/.gitignore`).  
- Don’t paste private keys into issues/chat.  
- Rotate with `./scripts/setup-wireguard.sh --rotate` and redistribute the player conf.

## Why not only a custom UDP proto?

Couchlink uses binary **`CLPD`** on the DataChannel for pads. Video still rides
WebRTC/SRTP for congestion control and encryption. WireGuard + WebRTC is the
low-risk HD default for a private path.
