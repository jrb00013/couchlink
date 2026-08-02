# Windows Privileged Helper Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** After one elevated `CouchlinkHelper-Setup.exe` install, WSL/`run.sh --online` and `--unblock-firewall` perform firewall/portproxy/UPnP prep via a LocalSystem service with **no UAC**.

**Architecture:** `couchlink-helper.exe` Windows service owns `\\.\pipe\couchlink-helper` (JSON-lines). Non-elevated `call-helper.ps1` is the only client; bash scripts prefer helper → legacy Scheduled Task → optional `COUCHLINK_ALLOW_UAC=1` RunAs. Inno Setup installs the service once.

**Tech Stack:** Rust (`windows-service`, `windows`, serde/clap), PowerShell pipe client, Inno Setup 6, bash integration in existing scripts.

**Spec:** `docs/superpowers/specs/2026-08-02-windows-privileged-helper-design.md`

## Global Constraints

- Never auto-pop UAC unless `COUCHLINK_ALLOW_UAC=1`.
- Allowlisted ops only: `ping`, `online_prep`, `firewall_unblock`.
- Reuse existing `enable-upnp.ps1` / `unblock-firewall.ps1` (installed next to the service binary).
- Pipe is local-only; no LAN TCP control plane in v1.
- Windows-only binary; protocol unit tests may run on Linux.

---

## File map

| Path | Role |
|------|------|
| `docs/superpowers/specs/2026-08-02-windows-privileged-helper-design.md` | Design |
| `crates/windows-helper/Cargo.toml` | Crate manifest |
| `crates/windows-helper/src/main.rs` | CLI entry (`install`/`uninstall`/`run`/`service`) |
| `crates/windows-helper/src/protocol.rs` | JSON request/response + tests |
| `crates/windows-helper/src/pipe_server.rs` | Named pipe server + ACL |
| `crates/windows-helper/src/ops.rs` | Dispatch ops → PowerShell scripts |
| `crates/windows-helper/src/service.rs` | Windows service glue |
| `Cargo.toml` | Workspace member |
| `scripts/windows/call-helper.ps1` | Non-elevated pipe client |
| `scripts/lib-windows-helper.sh` | Prefer helper / task / UAC gate |
| `scripts/enable-upnp.sh` | Use lib-windows-helper |
| `scripts/unblock-firewall.sh` | Use lib-windows-helper |
| `scripts/run.sh` | Messaging |
| `packaging/windows/couchlink-helper.iss` | Elevated installer |
| `packaging/windows/build-helper-installer.ps1` | Build exe + ISCC |
| `docs/NO_COMPUTER_UX.md`, `docs/PLAY_TOGETHER.md`, `docs/MESH.md` | Operator docs |

---

### Task 1: Protocol crate skeleton + unit tests

**Files:**
- Create: `crates/windows-helper/Cargo.toml`
- Create: `crates/windows-helper/src/protocol.rs`
- Create: `crates/windows-helper/src/lib.rs` (export protocol for tests)
- Create: `crates/windows-helper/src/main.rs` (stub `fn main` printing help)
- Modify: `Cargo.toml` (workspace members)

- [x] **Step 1: Add workspace member and crate**
- [x] **Step 2: Implement protocol types**
- [x] **Step 3: Unit tests (run on Linux CI)**
- [x] **Step 4: Verify**
- [ ] **Step 5: Commit**
---

### Task 2: Ops dispatcher (invoke installed PowerShell scripts)

**Files:**
- Create: `crates/windows-helper/src/ops.rs`
- Modify: `crates/windows-helper/src/lib.rs`

**Interfaces:**
- Consumes: `Request` from protocol
- Produces: `fn handle_request(req: &Request, script_dir: &Path) -> Response`

- [ ] **Step 1: Implement `handle_request`**

- `Ping` → `ok: true`, `version` from `env!("CARGO_PKG_VERSION")`
- `OnlinePrep` → run:
  `powershell.exe -NoProfile -ExecutionPolicy Bypass -File "{script_dir}\enable-upnp.ps1" [-SkipMap] [-WslIp …] [-SignalingPort …] [-TurnPort …]`
  Read exit code; map to Response (`exit` field). Prefer reading `%LOCALAPPDATA%\couchlink-run\enable-upnp.exit` when present (SYSTEM may use a fixed run dir under ProgramData — pass `-RunDir` to match; extend `enable-upnp.ps1` if needed to accept `-RunDir` already present).
- `FirewallUnblock` → run `unblock-firewall.ps1`; exit from marker or process.

On non-Windows, `handle_request` returns `ok: false, error: "windows only"` (so Linux unit tests for dispatch can stub).

- [ ] **Step 2: Unit test Ping path on all OS**

```rust
#[test]
fn ping_ok() {
    let resp = handle_request(&Request::Ping, Path::new("."));
    assert!(resp.ok);
    assert_eq!(resp.version.as_deref(), Some(env!("CARGO_PKG_VERSION")));
}
```

- [ ] **Step 3: Commit**

```bash
git add crates/windows-helper
git commit -m "$(cat <<'EOF'
feat: dispatch helper ops to PowerShell scripts

EOF
)"
```

---

### Task 3: Named pipe server + console `run` mode (Windows)

**Files:**
- Create: `crates/windows-helper/src/pipe_server.rs`
- Modify: `crates/windows-helper/Cargo.toml` (add `windows` features: `Win32_System_Pipes`, `Win32_Security`, `Win32_Foundation`, …)
- Modify: `crates/windows-helper/src/main.rs`

**Interfaces:**
- Produces: `fn serve_pipe(pipe_name: &str, script_dir: &Path) -> anyhow::Result<()>` — accept loop, one client at a time OK for v1

- [ ] **Step 1: Implement pipe server**

- Pipe name: `\\.\pipe\couchlink-helper`
- Create with security descriptor allowing Administrators + Interactive (document; refine to installing user SID in Task 5 if time)
- Read line → `parse_request_line` → `handle_request` → write response line + `\n`
- Log to `%ProgramData%\Couchlink\helper.log` (best-effort)

- [ ] **Step 2: CLI**

```text
couchlink-helper run [--script-dir DIR]   # foreground server (dev)
couchlink-helper ping-client               # optional self-test connect
```

- [ ] **Step 3: Manual smoke on Windows** (developer machine)

```powershell
# terminal A
.\target\release\couchlink-helper.exe run --script-dir $PWD\scripts\windows
# terminal B
.\scripts\windows\call-helper.ps1 -Op ping   # Task 4 may land first; or use a one-liner client
```

- [ ] **Step 4: Commit**

```bash
git add crates/windows-helper
git commit -m "$(cat <<'EOF'
feat: named pipe server for couchlink-helper

EOF
)"
```

---

### Task 4: PowerShell client + bash library

**Files:**
- Create: `scripts/windows/call-helper.ps1`
- Create: `scripts/lib-windows-helper.sh`
- Modify: `scripts/enable-upnp.sh`
- Modify: `scripts/unblock-firewall.sh`
- Modify: `scripts/test-mesh.sh` (bash -n new scripts)

**Interfaces:**
- `couchlink_helper_ping` → 0 if service answers
- `couchlink_helper_online_prep [--skip-map]` → exit code 0/2/other
- `couchlink_helper_firewall_unblock` → 0/1

- [ ] **Step 1: `call-helper.ps1`**

```powershell
param(
  [Parameter(Mandatory)][ValidateSet('ping','online_prep','firewall_unblock')][string]$Op,
  [switch]$SkipMap,
  [string]$WslIp = "",
  [int]$SignalingPort = 8443,
  [int]$TurnPort = 3478
)
# Connect NamedPipeClientStream to couchlink-helper, write JSON, read JSON, exit with code
```

- [ ] **Step 2: `lib-windows-helper.sh`**

Implement preference order from spec. Export functions used by enable-upnp / unblock-firewall.

- [ ] **Step 3: Wire `enable-upnp.sh`**

At top of elevate logic: if `couchlink_helper_online_prep` succeeds (or returns 0/2), skip task/UAC. Else fall through existing task → UAC only if `COUCHLINK_ALLOW_UAC=1`.

- [ ] **Step 4: Wire `unblock-firewall.sh` similarly**

- [ ] **Step 5: `bash -n` + `./scripts/test-mesh.sh`**

- [ ] **Step 6: Commit**

```bash
git add scripts/
git commit -m "$(cat <<'EOF'
feat: prefer Couchlink helper service over UAC for Windows prep

EOF
)"
```

---

### Task 5: Windows service install/uninstall + Inno installer

**Files:**
- Create: `crates/windows-helper/src/service.rs`
- Modify: `crates/windows-helper/src/main.rs` (`install` / `uninstall` / service main)
- Create: `packaging/windows/couchlink-helper.iss`
- Create: `packaging/windows/build-helper-installer.ps1`
- Modify: docs listed in file map

- [ ] **Step 1: Service registration**

Using `windows-service` crate:

- Service name: `CouchlinkHelper`
- Display: `Couchlink Helper`
- On start: `serve_pipe`
- `couchlink-helper install` → create service pointing at exe with `service` subcommand
- `couchlink-helper uninstall` → stop + delete

- [ ] **Step 2: Inno script**

`PrivilegesRequired=admin`, copy exe + PS1 scripts, `[Run]` → `couchlink-helper.exe install` and start service. Uninstall run → `uninstall`.

- [ ] **Step 3: `build-helper-installer.ps1`**

`cargo build -p couchlink-windows-helper --release` then ISCC.

- [ ] **Step 4: Docs**

Update `NO_COMPUTER_UX.md`, `PLAY_TOGETHER.md`, `MESH.md`: host on WSL needs **Couchlink Helper** setup once.

- [ ] **Step 5: Commit**

```bash
git add crates/windows-helper packaging/windows docs
git commit -m "$(cat <<'EOF'
feat: Windows service + Inno installer for Couchlink Helper

EOF
)"
```

---

### Task 6: End-to-end verification + PR

- [ ] **Step 1: On Windows/WSL host**

1. Build installer (or `couchlink-helper install` from elevated PowerShell).
2. Confirm `Get-Service CouchlinkHelper` is Running.
3. From WSL: `powershell.exe -File …\call-helper.ps1 -Op ping` → ok.
4. `./scripts/enable-upnp.sh --skip-map` → logs “via helper (no UAC)”.
5. Stop service → enable-upnp prints install hint; with `COUCHLINK_ALLOW_UAC=1` old path still works.

- [ ] **Step 2: Push + PR**

```bash
git push -u origin HEAD
gh pr create --title "feat: Windows privileged helper (zero UAC on run)" --body "..."
```

---

## Test plan

- [ ] `cargo test -p couchlink-windows-helper` (Linux OK for protocol)
- [ ] `bash -n scripts/lib-windows-helper.sh scripts/enable-upnp.sh scripts/unblock-firewall.sh`
- [ ] `./scripts/test-mesh.sh`
- [ ] Manual: helper install → `--online` without UAC
- [ ] Manual: uninstall → clean failure message

## Spec coverage checklist

| Spec item | Task |
|-----------|------|
| Service + named pipe | 3, 5 |
| Ops ping / online_prep / firewall_unblock | 1, 2 |
| call-helper.ps1 + bash prefer order | 4 |
| No auto UAC without escape hatch | 4 |
| Inno elevated installer | 5 |
| Docs | 5 |
| MSI/GPO phase 2 | deferred (called out in design only) |
| WireGuard op | deferred |
