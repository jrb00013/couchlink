# Headscale (self-hosted Tailscale control) — couchlink PRIME mesh

Friends never create a Tailscale Inc account. The **host** runs [Headscale](https://github.com/juanfont/headscale)
plus an embedded **DERP** relay; the join URL carries `hs=` (control URL) and `tskey=` (preauth key).
The friend client runs headless:

```bash
tailscale up --login-server="$hs" --auth-key="$tskey"
```

## Host

```bash
./install.sh --host --online
# or:
./scripts/setup-headscale.sh
./scripts/enable-headscale.sh
./scripts/run.sh host --online
```

`enable-headscale.sh` will:

1. Start Headscale on `:8080`
2. Publish it with **cloudflared** HTTPS (required for embedded DERP TLS)
3. Mint host + player preauth keys under `infra/headscale/` (gitignored)
4. Join the host Tailscale client to this control plane
5. Export `COUCHLINK_MESH=headscale`, mesh IP, `COUCHLINK_HS_URL`, `COUCHLINK_TS_AUTHKEY`

Override public URL: `COUCHLINK_HS_URL=https://hs.example.com`

## Friend

```bash
./install.sh --online
# paste the host join URL (includes hs= + tskey=)
# optional:
./scripts/run.sh client --online --unblock-firewall
```

## Invite params

| Param | Meaning |
|-------|---------|
| `mesh=headscale` | Auto-join Headscale |
| `hs=` | HTTPS control URL |
| `tskey=` | Preauth key |

## Fallback

If Headscale fails, `host --online` still tries Tailscale cloud / WireGuard / Cloudflare as before.

## Ops notes

- Treat join URLs as secrets (auth keys).
- STUN for embedded DERP uses **UDP 3479** (3478 stays couchlink TURN).
- See design: `docs/superpowers/specs/2026-08-01-headscale-mesh-design.md`
