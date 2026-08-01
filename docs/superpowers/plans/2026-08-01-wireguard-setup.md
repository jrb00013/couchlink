# WireGuard setup for couchlink — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make the optional WireGuard path a first-class, documented, mostly-automatic setup so `install.sh` (opt-in) installs tools and generates host/player configs, and docs walk host + friend from zero to a LAN-style join over `10.66.0.x`.

**Architecture:** Keep today’s public STUN / `--online` (UPnP → Cloudflare + IPv6 → bore) as the zero-install default. WireGuard is an optional private mesh: generate gitignored keys + `wg0-*.conf` from the existing examples, bring up with explicit `wg-quick up` (never during install), then run couchlink as if on LAN (`ws://10.66.0.1:8443`). No WebRTC ICE code changes in this pass beyond documenting join URL / `COUCHLINK_ICE_IPS`.

**Tech Stack:** WireGuard (`wireguard-tools` / `wg-quick`), bash (`install.sh`, new `scripts/setup-wireguard.sh`), existing `infra/wireguard/*.example`, markdown runbook in `docs/WIREGUARD.md`.

## Global Constraints

- Opt-in only: `COUCHLINK_INSTALL_WIREGUARD=1` (default off); standalone `./scripts/setup-wireguard.sh` always available.
- Never commit private keys or generated `wg0-*.conf` — gitignore `infra/wireguard/keys/` and `wg0-*.conf` (keep `*.example`).
- Do **not** auto-`wg-quick up` in `install.sh` (needs root; wrong Endpoint can break networking).
- No Tailscale integration this pass (docs may mention it as an alternative).
- No forcing WG on every install; no putting secrets in `.env.couchlink` by default.
- Spike WSL layout before promising install auto-magic on WSL (Windows WireGuard app preferred over WG-inside-WSL2).

## Why

UPnP is often disabled; Cloudflare quick tunnels work but are not “direct to your PC.” WireGuard gives a private mesh so signaling and media look like LAN without public TURN/UPnP for the couchlink ports. Handshake still needs UDP **51820** reachable (or a relay VPS) — same class of NAT problem, but only once for the VPN.

## Non-goals (this pass)

- Tailscale (or other mesh) product integration
- Changing WebRTC ICE selection beyond documentation
- `./scripts/run.sh host --wireguard` convenience flag (optional follow-up, Task 5)
- Auto-starting the tunnel during install

## Current state

| Piece | Status |
|-------|--------|
| `docs/WIREGUARD.md` | Stub (4 steps) |
| `infra/wireguard/*.example` | Minimal host/player confs (`10.66.0.1` / `10.66.0.2`, port 51820) |
| `install.sh` | No `wireguard-tools` |
| Runtime | `--local` / `--online` ignore WG |

---

### Task 1: Full WireGuard runbook (`docs/WIREGUARD.md`)

**Files:**
- Modify: `docs/WIREGUARD.md` (replace stub)
- Modify: `infra/wireguard/README.md` (point at setup script + docs)
- Modify: `README.md` (one line under Docs / Run pointing at WG path)

**Interfaces:**
- Consumes: existing example confs and current `--local` / `--online` behavior
- Produces: operator runbook agents and humans follow before/after Task 2–3

- [ ] **Step 1: Rewrite `docs/WIREGUARD.md` with these sections**

  1. **When to use WG** vs `--online` / Cloudflare / IPv6 / manual port forward  
  2. **Prerequisites** — UDP 51820 inbound to host (UPnP/manual/VPS relay); friend has `PersistentKeepalive = 25` so NAT on the player side is fine; host still needs a reachable Endpoint unless using a relay  
  3. **Host flow** — `COUCHLINK_INSTALL_WIREGUARD=1 ./install.sh` or `./scripts/setup-wireguard.sh` → firewall allow 51820/udp → `sudo wg-quick up wg0` (or Windows WireGuard import) → `./scripts/run.sh host --local` (invite on `10.66.0.1`)  
  4. **Friend flow** — install WireGuard → import `wg0-player.conf` → `wg-quick up` → open `http://10.66.0.1:8443/?s=…&p=…` or native client with that join URL  
  5. **WebCodecs note** — browser over `http://10.66.0.x` is not a secure context; prefer **native client**, or SSH/local tunnel to `http://127.0.0.1:8443`  
  6. **Troubleshooting** — handshake fail (Endpoint/firewall), `AllowedIPs`, ping `10.66.0.1`/`10.66.0.2`, wrong interface  
  7. **WSL layouts (call out hard)**  
     - **A (recommended on WSL):** WireGuard on **Windows**; friend ↔ Windows; couchlink in WSL must be reachable on the WG subnet (may need route/portproxy — mark as “spike before coding install auto-magic”)  
     - **B:** Native Linux host — WG + couchlink on same machine (simplest)  
  8. **Security** — keys stay under `infra/wireguard/keys/` (gitignored); never paste private keys into chat/issues; rotate with `./scripts/setup-wireguard.sh --rotate`  
  9. **Tailscale** — one short paragraph: same “private LAN” idea, less key exchange, not wired into couchlink  

- [ ] **Step 2: Trim `infra/wireguard/README.md` to point at the runbook + setup script**

```markdown
# WireGuard examples for couchlink

Templates: `wg0-host.conf.example`, `wg0-player.conf.example`.

Generate real configs (gitignored): `./scripts/setup-wireguard.sh`

Full runbook: [`docs/WIREGUARD.md`](../../docs/WIREGUARD.md).
```

- [ ] **Step 3: Add one README.md Docs bullet**

Under Docs (or Run), add: `- [WireGuard](docs/WIREGUARD.md) — optional private mesh when UPnP/Cloudflare is not enough`

- [ ] **Step 4: Commit**

```bash
git add docs/WIREGUARD.md infra/wireguard/README.md README.md
git commit -m "$(cat <<'EOF'
docs: expand WireGuard runbook and point install path

EOF
)"
```

---

### Task 2: `setup-wireguard.sh` + gitignore

**Files:**
- Create: `scripts/setup-wireguard.sh`
- Create: `infra/wireguard/.gitignore`
- Modify: `.env.example` (document `COUCHLINK_INSTALL_WIREGUARD` and optional public IP for Endpoint)

**Interfaces:**
- Consumes: `infra/wireguard/wg0-host.conf.example`, `wg0-player.conf.example`; optional `COUCHLINK_PUBLIC_IP` / `ifconfig.me`
- Produces: `infra/wireguard/keys/{host,player}.{key,pub}`; `infra/wireguard/wg0-host.conf`; `infra/wireguard/wg0-player.conf`; stdout instructions

- [ ] **Step 1: Add `infra/wireguard/.gitignore`**

```gitignore
keys/
wg0-host.conf
wg0-player.conf
*.key
*.pub
```

- [ ] **Step 2: Implement `scripts/setup-wireguard.sh` (idempotent)**

Behavior:

1. Require `wg` / `wg genkey` on PATH (print install hint: `apt install wireguard` / `brew install wireguard-tools`).
2. `mkdir -p infra/wireguard/keys`.
3. If `keys/host.key` missing (or `--rotate`): `umask 077`; `wg genkey | tee host.key | wg pubkey > host.pub` (same for player).
4. Resolve Endpoint host: `COUCHLINK_PUBLIC_IP` → else `curl -fsS --max-time 5 ifconfig.me` → else literal `HOST_PUBLIC_IP` placeholder.
5. Write `wg0-host.conf` / `wg0-player.conf` by substituting into the examples (Address, ListenPort 51820, PrivateKey, Peer PublicKey, Endpoint, AllowedIPs, PersistentKeepalive on player).
6. Print next steps: copy player conf to friend; open UDP 51820; `sudo wg-quick up wg0` (or Windows import); then `./scripts/run.sh host --local` and join via `http://10.66.0.1:8443/…`.
7. Flags: `--rotate` regenerates keys + confs; `--help` usage.

Skeleton:

```bash
#!/usr/bin/env bash
# Generate couchlink WireGuard keys + wg0-host/player.conf (gitignored).
# Does not bring the tunnel up — see docs/WIREGUARD.md.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
WG_DIR="$ROOT/infra/wireguard"
KEYS="$WG_DIR/keys"
ROTATE=0
# … parse --rotate / -h …
command -v wg >/dev/null || { echo "install wireguard-tools first" >&2; exit 1; }
mkdir -p "$KEYS"
chmod 700 "$KEYS"
gen_pair() {
  local name="$1"
  if [[ "$ROTATE" == 1 || ! -f "$KEYS/$name.key" ]]; then
    umask 077
    wg genkey | tee "$KEYS/$name.key" | wg pubkey > "$KEYS/$name.pub"
    chmod 600 "$KEYS/$name.key"
  fi
}
gen_pair host
gen_pair player
# … write confs from examples with sed/envsubst …
```

- [ ] **Step 3: Document env in `.env.example`**

```bash
# Optional: install WireGuard tools + generate configs during ./install.sh (default off).
#COUCHLINK_INSTALL_WIREGUARD=1
# Used as WireGuard Endpoint host when running setup-wireguard.sh
#COUCHLINK_PUBLIC_IP=
```

- [ ] **Step 4: Manual smoke (no commit of secrets)**

```bash
./scripts/setup-wireguard.sh
./scripts/setup-wireguard.sh   # second run: same keys
grep -E 'PrivateKey|HOST_PUBLIC' infra/wireguard/wg0-host.conf infra/wireguard/wg0-player.conf
git status --short infra/wireguard/   # must not list keys/*.key as tracked candidates if gitignored
git check-ignore -v infra/wireguard/keys/host.key infra/wireguard/wg0-host.conf
```

Expected: confs exist; `git check-ignore` reports ignore rules; working tree does not stage secrets.

- [ ] **Step 5: Commit**

```bash
git add scripts/setup-wireguard.sh infra/wireguard/.gitignore .env.example
git commit -m "$(cat <<'EOF'
feat: add setup-wireguard.sh for opt-in mesh configs

EOF
)"
```

---

### Task 3: Opt-in hook in `install.sh`

**Files:**
- Modify: `install.sh` (apt/brew `wireguard` / `wireguard-tools`; call setup when opted in)

**Interfaces:**
- Consumes: `COUCHLINK_INSTALL_WIREGUARD`; `scripts/setup-wireguard.sh`
- Produces: tools on PATH after install; generated confs when flag set; printed pointer to `docs/WIREGUARD.md`

- [ ] **Step 1: Linux/WSL apt package**

In the existing `apt-get install` list (or a follow-up block when `COUCHLINK_INSTALL_WIREGUARD=1`), install `wireguard` / `wireguard-tools` (Debian/Ubuntu package name is typically `wireguard`). Keep default install light: only when flag is `1`, **or** always install tools but only run setup when flagged — prefer **tools + setup only when flag is 1** so normal installs stay unchanged.

- [ ] **Step 2: macOS brew**

When flag is `1` and brew exists: `brew install wireguard-tools` (and note macOS host is video-only for pad injection; WG still fine for client path).

- [ ] **Step 3: Call setup**

After cargo/UI steps (or after deps), if `COUCHLINK_INSTALL_WIREGUARD=1`:

```bash
if [[ "${COUCHLINK_INSTALL_WIREGUARD:-0}" == "1" ]]; then
  echo "==> WireGuard: generating configs (opt-in)"
  run_as_user "$ROOT/scripts/setup-wireguard.sh" || \
    echo "warning: setup-wireguard.sh failed — see docs/WIREGUARD.md"
  echo "    bring-up is manual: sudo wg-quick up wg0  (see docs/WIREGUARD.md)"
fi
```

- [ ] **Step 4: Verify default path unchanged**

```bash
# Without flag: install output must not mention generating WireGuard configs
COUCHLINK_INSTALL_WIREGUARD=0 ./install.sh   # or dry-read the branch
```

- [ ] **Step 5: Commit**

```bash
git add install.sh
git commit -m "$(cat <<'EOF'
feat: opt-in WireGuard tools + config generation in install.sh

EOF
)"
```

---

### Task 4: WSL spike + doc correction

**Files:**
- Modify: `docs/WIREGUARD.md` (WSL section accuracy after spike)
- Optionally modify: `scripts/setup-wireguard.sh` (print Windows-specific copy path / `.conf` import hint when `couchlink_detect_platform` is `wsl`)

**Interfaces:**
- Consumes: WSL2 + Windows WireGuard app; existing WSL portproxy patterns in `scripts/windows/enable-upnp.ps1`
- Produces: documented working layout A or “gate auto-setup on WSL”

- [ ] **Step 1: Spike layout A**

On this WSL host: import `wg0-host.conf` into Windows WireGuard; from a second peer (or phone WG), handshake; confirm whether WSL processes are reachable at `10.66.0.1:8443` or only the Windows host is. Record: need mirrored networking, `netsh portproxy`, or bind couchlink on Windows-reachable address.

- [ ] **Step 2: Spike layout B only if a native Linux box is available**

Confirm simplest path: WG + `./scripts/run.sh host --local` + friend join `http://10.66.0.1:8443/…`.

- [ ] **Step 3: Update docs from spike results**

Rewrite the WSL section with the layout that worked. If layout A needs extra portproxy, document exact commands. If WG-inside-WSL is broken, state “setup script prints configs; bring-up on Windows” and do **not** claim `install.sh` brings the tunnel up on WSL.

- [ ] **Step 4: Commit**

```bash
git add docs/WIREGUARD.md scripts/setup-wireguard.sh
git commit -m "$(cat <<'EOF'
docs: record WSL WireGuard bring-up after spike

EOF
)"
```

---

### Task 5 (optional follow-up): `run.sh host --wireguard`

**Files:**
- Modify: `scripts/run.sh`
- Modify: `docs/WIREGUARD.md`

**Interfaces:**
- Consumes: `wg0` up with `10.66.0.1`; existing `--local` join URL machinery
- Produces: mode that waits for `wg0`, sets invite/signaling to `10.66.0.1:8443`, skips Cloudflare/UPnP online fallback

- [ ] **Step 1: Add `--wireguard` flag** alongside `--local` / `--online`
- [ ] **Step 2: If `wg show wg0` fails, print “run setup-wireguard.sh && wg-quick up” and exit non-zero**
- [ ] **Step 3: Reuse local-mode stack (no TURN required on mesh) but force join URL host `10.66.0.1`**
- [ ] **Step 4: Document in WIREGUARD.md; commit**

Defer until Tasks 1–4 are done unless a host urgently needs the flag.

---

## Test plan (acceptance)

1. **Linux VM pair:** `setup-wireguard.sh` → `wg-quick up` both sides → `ping 10.66.0.2` → host `--local` → client/browser joins via `http://10.66.0.1:8443/?s=…&p=…` (native client preferred for WebCodecs).
2. **Secrets:** `git status` never shows `keys/*.key` as tracked; `git check-ignore` covers confs + keys.
3. **Idempotent setup:** second `./scripts/setup-wireguard.sh` keeps the same keys; `--rotate` changes them.
4. **Default install:** without `COUCHLINK_INSTALL_WIREGUARD=1`, `install.sh` behavior unchanged.
5. **WSL:** docs match spike; no false claim that install brings up WG inside WSL2.

## Suggested implementation order

1. Task 1 — docs rewrite (can ship alone)  
2. Task 2 — `setup-wireguard.sh` + gitignore  
3. Task 3 — `install.sh` opt-in  
4. Task 4 — WSL spike / doc correction  
5. Task 5 — optional `--wireguard` run mode  

## Related context

- Current online fallback when UPnP fails: Cloudflare HTTPS invite + IPv6 TURN (`scripts/run.sh`, `scripts/lib-online-tunnel.sh`).
- Existing examples: `infra/wireguard/wg0-host.conf.example`, `wg0-player.conf.example`.
- Play guide: `docs/PLAY_TOGETHER.md` (link WG as an alternative path when editing docs).
