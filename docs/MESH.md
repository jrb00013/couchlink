# Mesh path (Tailscale + WireGuard) — PRIME for `--online`

When your router won’t do UPnP and IPv6 isn’t an option for your friend,
**Tailscale** or **WireGuard** is the intended way to play across the globe.
Cloudflare / UPnP / IPv6 TURN remain automatic **fallbacks** if no mesh is up.

## Priority (`./scripts/run.sh host --online`)

1. **Tailscale** — if `tailscale ip -4` returns a `100.x` address  
2. **WireGuard** — if `wg0` (Linux) or Windows WireGuard tunnel is up (`10.66.0.1` by default)  
3. **UPnP + public IPv4 TURN**  
4. **Cloudflare HTTPS** invite + **IPv6 TURN** (or IPv4 TURN warning)  
5. **bore** signaling-only last resort  

Mesh sessions skip Cloudflare. On **native Linux/macOS**, TURN is skipped (direct ICE on the mesh iface). On **WSL**, TURN stays on `turn:MESH_IP:3478` because WebRTC UDP is not covered by WSL portproxy — friends still get a working media path.

Override order: `COUCHLINK_MESH_PREFER=wireguard,tailscale`  
Skip mesh entirely: `COUCHLINK_SKIP_MESH=1`

## Quick start — Tailscale (easiest)

**Host**

```bash
./scripts/setup-tailscale.sh          # install hints + status
# sign in (Windows app / sudo tailscale up)
./scripts/setup-tailscale.sh --check  # must print 100.x
./scripts/run.sh host --online        # prints http://100.x.y.z:8443/?…
```

**Friend**

1. Install Tailscale and join the **same tailnet** (host can share the node).  
2. Paste the host join URL into **Couchlink Player** (native client preferred).  
3. Browser over `http://100.x` works for signaling/RTP fallback; WebCodecs wants https — use the native client.

## Quick start — WireGuard

```bash
./install.sh --online
# or step-by-step:
./scripts/setup-wireguard.sh
./scripts/enable-wireguard.sh      # Windows UAC once, then Task Scheduler; Linux: wg-quick
./scripts/run.sh host --online     # auto-calls enable-wireguard when conf exists
```

`host --online` now **brings the tunnel up** when `infra/wireguard/wg0-host.conf` exists (`COUCHLINK_AUTO_WIREGUARD=0` to skip).

Friend imports `infra/wireguard/wg0-player.conf`, brings the tunnel up, opens the join URL.

WireGuard still needs **UDP 51820** reachable on the host (or a relay). Tailscale handles NAT traversal for you.

## Install opt-in

```bash
COUCHLINK_INSTALL_MESH=1 ./install.sh
```

Installs `wireguard-tools` (apt/brew), runs `setup-wireguard.sh` + `setup-tailscale.sh` hints.
Does **not** auto-login Tailscale or `wg-quick up`.

## Client

```bash
./scripts/run.sh client --online
# paste the mesh join URL from the host (often no turn= query params)
```

## WSL notes

- **Tailscale:** prefer the **Windows** Tailscale app; `run.sh` looks for `tailscale.exe`.  
- **WireGuard:** prefer the **Windows** WireGuard app with `wg0-host.conf`. If the tunnel lives only on Windows while couchlink runs in WSL, set `COUCHLINK_WG_HOST_IP=10.66.0.1` (and ensure Windows can reach WSL `:8443`, or run signaling bound accordingly). Full WSL layout notes: [WIREGUARD.md](WIREGUARD.md).

## Related

- [WIREGUARD.md](WIREGUARD.md) — keys, confs, bring-up, troubleshooting  
- [PLAY_TOGETHER.md](PLAY_TOGETHER.md) — general host/friend flow  
- Scripts: `scripts/lib-mesh.sh`, `scripts/setup-tailscale.sh`, `scripts/setup-wireguard.sh`
