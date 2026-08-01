# Headscale Mesh Auto-Join Implementation Plan

> **For agentic workers:** Implement task-by-task. Steps use checkbox (`- [ ]`) syntax.

**Goal:** Automate Headscale + self-hosted DERP mesh so `./install.sh` / `--run|--online` make host and friend paste-link join work without Tailscale Inc accounts; add `--unblock-firewall` for client.

**Architecture:** Host runs Headscale (public `hs=` via cloudflared or WAN), mints preauth keys, embeds `mesh=headscale&hs=&tskey=` in invite. Friend client headlessly `tailscale up --login-server --auth-key`. Optional OS firewall unblock dispatcher.

**Tech Stack:** Headscale binary, optional derper, existing cloudflared helper, bash/PowerShell, Rust invite encode/parse, Tailscale client.

## Global Constraints

- Default `./install.sh` = client; `./install.sh --host` = host (unchanged).
- Secrets under `infra/headscale/` gitignored.
- Do not remove Tailscale-cloud / WireGuard / Cloudflare fallbacks.
- Prefer Headscale in `COUCHLINK_MESH_PREFER` when Headscale is up.

---

## File map

| Path | Role |
|------|------|
| `docs/superpowers/specs/2026-08-01-headscale-mesh-design.md` | Design |
| `docs/HEADSCALE.md` | Operator docs |
| `docs/MESH.md` | Point at Headscale as PRIME |
| `infra/headscale/config-example.yaml` | Example config |
| `infra/headscale/.gitignore` | Ignore runtime secrets |
| `scripts/lib-headscale.sh` | Detect/start/key helpers |
| `scripts/setup-headscale.sh` | Install binary + init |
| `scripts/enable-headscale.sh` | Bring up HS + DERP + host join |
| `scripts/unblock-firewall.sh` | Dispatcher |
| `scripts/windows/unblock-firewall.ps1` | Windows/WSL firewall |
| `scripts/lib-mesh.sh` | Prefer headscale; export invite env |
| `scripts/run.sh` | `--unblock-firewall`; host enable HS |
| `scripts/setup-tailscale.sh` | Headless up with login-server + key |
| `install.sh` | Host: setup-headscale; client: unchanged Tailscale client |
| `crates/host/src/invite.rs` | `hs`, `tskey` params |
| `crates/client/src/invite.rs` | Parse + detect headscale |
| `crates/client/src/main.rs` | Trigger join helper / env |
| `scripts/start-client.sh` / `start-host.sh` | Pass through env |
| `.env.example` | New vars |
| `scripts/test-mesh.sh` | Headscale override smoke |

---

### Task 1: Docs + gitignore + example config

- [x] Write `docs/HEADSCALE.md`
- [x] Update `docs/MESH.md` PRIME order
- [x] `infra/headscale/.gitignore` + `config-example.yaml`

### Task 2: Headscale setup/enable scripts

- [x] `scripts/setup-headscale.sh` — download Headscale release to `.tools/`, write config
- [x] `scripts/lib-headscale.sh` — start HS, create user, mint preauth key, optional cloudflared for `hs` URL, host `tailscale up --login-server`
- [x] `scripts/enable-headscale.sh` — orchestrate bring-up; export `COUCHLINK_HS_URL`, `COUCHLINK_TS_AUTHKEY`, `COUCHLINK_MESH=headscale`, mesh IP
- [x] Embedded DERP in Headscale config (server URL public)

### Task 3: Mesh prefer + invite encode/parse

- [x] `lib-mesh.sh`: prefer `headscale` when enabled; set invite env `COUCHLINK_HS_URL` / `COUCHLINK_TS_AUTHKEY`
- [x] Host `player_invite_url` adds `hs` + `tskey`
- [x] Client parse `hs`/`tskey`; `is_headscale_invite`
- [x] Unit tests

### Task 4: Friend headless join

- [x] `scripts/join-headscale.sh` — ensure Tailscale client, `up --login-server --auth-key`
- [x] `run.sh client` / start-client: if join URL or env has hs+tskey, call join script
- [x] Client Rust: on headscale invite, log + set env / invoke via documented run.sh path (script-primary for privileges)

### Task 5: `--unblock-firewall`

- [x] `scripts/unblock-firewall.sh` + Windows PS1 + linux/macos stubs
- [x] `run.sh` / `install.sh` accept `--unblock-firewall` for client role

### Task 6: install.sh wiring + smoke + PR

- [x] Host install runs `setup-headscale.sh`; `--host --online` enables Headscale before mesh detect
- [x] `test-mesh.sh` headscale override
- [x] `.env.example` docs
- [ ] Push branch + `gh pr create`

## Test plan

- [ ] `bash -n` all new scripts
- [ ] `./scripts/test-mesh.sh`
- [ ] `cargo test -p couchlink-host invite` / `cargo test -p couchlink-client invite`
- [ ] Manual: host enable-headscale prints `hs=` URL (or dry-run with env override)
