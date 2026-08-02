# Mesh path (Headscale + Tailscale + WireGuard) — PRIME for `--online`

When your router won’t do UPnP and IPv6 isn’t an option for your friend,
**Headscale** (self-hosted), **Tailscale**, or **WireGuard** is the intended way
to play across the globe. Cloudflare / UPnP / IPv6 TURN remain automatic
**fallbacks** if no mesh is up.

## Priority (`./scripts/run.sh host --online`)

1. **Headscale** — host runs control plane + DERP; invite has `mesh=headscale&hs=&tskey=`  
2. **Tailscale** (Tailscale Inc) — if `tailscale ip -4` returns a `100.x` address  
3. **WireGuard** — if `wg0` (Linux) or Windows WireGuard tunnel is up (`10.66.0.1` by default)  
4. **UPnP + public IPv4 TURN**  
5. **Cloudflare HTTPS** invite + **IPv6 TURN** (or IPv4 TURN warning)  
6. **bore** signaling-only last resort  

Mesh sessions skip Cloudflare for signaling. On **native Linux/macOS**, TURN is skipped (direct ICE on the mesh iface). On **WSL**, TURN stays on `turn:MESH_IP:3478` because WebRTC UDP is not covered by WSL portproxy — friends still get a working media path.

Override order: `COUCHLINK_MESH_PREFER=tailscale,wireguard`  
Skip mesh entirely: `COUCHLINK_SKIP_MESH=1`  
Skip Headscale only: `COUCHLINK_SKIP_HEADSCALE=1`

## Quick start — Headscale (paste-link, no Tailscale Inc account for friends)

See **[docs/HEADSCALE.md](HEADSCALE.md)**.

**Host**

```bash
./install.sh --host --online          # enable-headscale + host; prints http://100.x…/?…&hs=…&tskey=…
```

**Friend**

```bash
./install.sh --online                 # paste URL → auto `tailscale up --login-server --auth-key`
./install.sh --online --unblock-firewall
```

## Quick start — Tailscale cloud (paste-link)

**Host (gaming PC)**

```bash
COUCHLINK_SKIP_HEADSCALE=1 ./install.sh --host --online
# or: ./scripts/setup-tailscale.sh --ensure && ./scripts/run.sh host --online
```

**Friend**

```bash
./install.sh                          # player + Tailscale
./install.sh --online                 # paste the host join URL
```

1. Sign into Tailscale (same tailnet as host / accept a share) — install already put Tailscale on the machine.  
2. Paste the host join URL into **Couchlink Player**.  
3. Browser over `http://100.x` works for signaling/RTP fallback; WebCodecs wants https — use the native client.

## Quick start — WireGuard

```bash
./install.sh --online
# or step-by-step:
./scripts/setup-wireguard.sh
./scripts/enable-wireguard.sh      # prefers Helper/task; else UAC if COUCHLINK_ALLOW_UAC=1; Linux: wg-quick
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

- **Couchlink Helper:** install `CouchlinkHelper-Setup.exe` once so firewall / WSL portproxy / UPnP prep need no UAC on `--online` (see [NO_COMPUTER_UX.md](NO_COMPUTER_UX.md)).  
- **Tailscale:** prefer the **Windows** Tailscale app; `./scripts/setup-tailscale.sh --ensure` installs it from WSL via winget/MSI (UAC once). `run.sh` looks for `tailscale.exe`.  
- **WireGuard:** prefer the **Windows** WireGuard app with `wg0-host.conf`. If the tunnel lives only on Windows while couchlink runs in WSL, set `COUCHLINK_WG_HOST_IP=10.66.0.1` (and ensure Windows can reach WSL `:8443`, or run signaling bound accordingly). Full WSL layout notes: [WIREGUARD.md](WIREGUARD.md).

## Related

- [WIREGUARD.md](WIREGUARD.md) — keys, confs, bring-up, troubleshooting  
- [PLAY_TOGETHER.md](PLAY_TOGETHER.md) — general host/friend flow  
- Scripts: `scripts/lib-mesh.sh`, `scripts/setup-tailscale.sh`, `scripts/setup-wireguard.sh`
