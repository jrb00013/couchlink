# Headscale mesh + auto-join + firewall unblock — Design

**Date:** 2026-08-01  
**Status:** Approved for implementation (user request)  
**Branch:** `feat/headscale-mesh-auto-join`

## Problem

Friends should not create Tailscale Inc accounts or click through Tailscale login.
Cross-globe play needs a mesh that works when UPnP is off. We want:

1. **Self-hosted control plane** (Headscale) + **self-hosted DERP** so we do not depend on Tailscale’s cloud coordination/DERP.
2. **One paste link** that configures the friend’s Tailscale-compatible client to the **host’s** Headscale URL and joins with a **preauth key** (headless).
3. **`./install.sh` / `./install.sh --run|--online`** automate host vs client roles.
4. **`--unblock-firewall`** on the client path opens local OS firewall rules (Windows, WSL, Linux, macOS).

## Non-goals (this pass)

- Replacing WebRTC/TURN entirely (mesh carries reachability; couchlink protocols stay).
- Multi-host shared Headscale SaaS (one Headscale per gaming PC / household is enough).
- Silent macOS firewall without admin approval (best-effort + prompts).

## Architecture

```
Friend                         Public bootstrap              Host
-----                         -----------------              ----
paste invite ──► hs= + tskey= ──► Headscale (HTTPS) ◄── tailscaled (host)
                 (Cloudflare or                   │
                  public IP:8080)                 ▼
                                            DERP (self-hosted)
                 mesh IP 100.x ◄──────── WireGuard paths ────────►
                 couchlink ws/turn on mesh IP (as today)
```

### Invite query params (additive)

| Param | Meaning |
|-------|---------|
| `mesh=headscale` | Use Headscale auto-join |
| `hs=` | Headscale base URL (`https://…`) |
| `tskey=` | Preauth key (ephemeral/reusable) |
| `derp=` | Optional DERP map hint / URL (else Headscale embeds/configures) |

Existing `s`, `p`, `ws`, `turn*` unchanged.

### Chicken-and-egg

Friend must reach Headscale **before** the mesh exists → `hs=` is always a **public** bootstrap URL (prefer cloudflared HTTPS; else host public IP + port).

### Host automation (`./install.sh --host` / `--host --online`)

1. Install Tailscale-compatible client (Windows app from WSL; Linux/macOS package).
2. Install/run **Headscale** (binary under `.tools/headscale` or container).
3. Ensure **DERP** (Headscale embedded DERP and/or `derper` sidecar).
4. Create namespace/user + preauth key; persist under `infra/headscale/` (gitignored secrets).
5. `tailscale up --login-server=<local-or-public-hs> --auth-key=<host-key>` on the host.
6. Prefer mesh invite: `COUCHLINK_MESH=headscale`, `COUCHLINK_MESH_IP=<host TS IP>`, embed `hs` + `tskey` in join URL.
7. Fall back to Tailscale cloud / WireGuard / Cloudflare if Headscale bring-up fails.

### Friend automation (`./install.sh` / `--online`)

1. Install Tailscale-compatible client (no account).
2. On run / paste join URL: if `mesh=headscale` (or `hs=`+`tskey=`), run headless:
   `tailscale up --login-server="$hs" --auth-key="$tskey" --hostname=couchlink-player-…`
3. Optional `--unblock-firewall` before join.

### `--unblock-firewall`

Dispatcher `scripts/unblock-firewall.sh` → platform scripts:

| Platform | Mechanism |
|----------|-----------|
| Windows | Elevated PowerShell: allow Tailscale + UDP/TCP 41641, 3478, 8443 |
| WSL | Windows firewall + portproxy (reuse enable-upnp patterns) |
| Linux | firewalld/ufw/nft best-effort |
| macOS | `socketfilterfw` / user prompts; document admin need |

## Security

- Preauth keys are **secrets**; treat join URLs like passwords. Prefer ephemeral + short TTL.
- Do not commit `infra/headscale/*.key` / preauth material (gitignore).
- Revoke keys from Headscale admin/CLI when sessions end (best-effort hook later).

## Why Headscale over Tailscale cloud (for this product)

| | Tailscale cloud + auth key | Headscale + self DERP |
|--|---------------------------|------------------------|
| Friend account | Not needed | Not needed |
| Host account | Required | Not required (self-host) |
| DERP | Tailscale fleet | We run it |
| Ops | Low | Medium |
| Independence | Low | High |

Product choice: **Headscale-first for PRIME mesh**, keep Tailscale cloud / WG / Cloudflare as fallbacks.

## Success criteria

1. Host: `./install.sh --host --online` prints join URL with `mesh=headscale&hs=…&tskey=…`.
2. Friend: `./install.sh --online`, paste URL → headless join → can reach host mesh IP.
3. `--unblock-firewall` runs without crashing on each supported OS (best-effort success).
4. CI: unit tests for invite parse/encode; mesh smoke includes headscale override.
