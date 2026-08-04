# Direct VPN + universal NAT traversal — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Status:** PLAN ONLY — nothing in this document has been implemented.

**Goal:** A direct host↔friend connection that works from *any* network, with no
third-party relay in the steady-state path, no router configuration, and no
assumption that either side has IPv6, a public IPv4, UPnP, or a cooperative ISP.
Relays stay available strictly as a last-resort fallback.

**Core principle:** every capability is *detected and used when present*, never
*required*. IPv6, UPnP, and port forwards are fast paths, not prerequisites.

---

## Why the current WireGuard path cannot work

Verified against the code, not assumed:

| # | Finding | Evidence |
|---|---------|----------|
| 1 | Endpoint is hardcoded to the host's IPv4 WAN address | `scripts/setup-wireguard.sh:44-48` — `COUCHLINK_PUBLIC_IP`, else `ifconfig.me`, else literal `HOST_PUBLIC_IP` |
| 2 | The host can never initiate a handshake | `wg0-host.conf` (lines 72-81) gives the peer no `Endpoint`, so only the player can start |
| 3 | Nothing ever opens UDP 51820 at the router | `enable-wireguard.ps1:78-86` adds a Windows *firewall* rule only; `grep -rn 51820` shows UPnP scripts are never asked for it |
| 4 | A dead tunnel silently beats working fallbacks | `couchlink_wireguard_ip` (`lib-mesh.sh:107-121`) checks only that the interface exists — `wg show <if> latest-handshakes` is never called, so `run.sh:272` skips Cloudflare/IPv6/bore and advertises an unreachable `10.66.0.x` URL |
| 5 | Docs promise a relay that does not exist | `docs/WIREGUARD.md:24` mentions "relay/VPS Endpoint"; no code implements it |
| 6 | UDP is not proxied into WSL despite the log | `enable-wireguard.ps1:131-146` logs "8443/3478" but `netsh interface portproxy` is **TCP-only**; comments at `lib-mesh.sh:138,167` repeat the wrong claim |

Net effect: the WireGuard path is a *"you already have a port forward"* path wearing
the clothes of a NAT-traversal solution. On this network — Spectrum Askey MAX2V1K,
read-only admin page, UPnP IGD disabled, external check confirming TCP 8080 closed —
it cannot ever complete a handshake.

---

## Scope: browser friends vs native friends

| Friend runs | Can join the VPN? | Path |
|-------------|-------------------|------|
| **Browser** (current setup) | **Yes — via a WASM WireGuard stack** | WireGuard-in-WASM over WebSocket/WebTransport, or WebRTC ICE |
| **Native client** | Yes | Native WireGuard tunnel |

**A browser can join a VPN.** The constraint is narrower than "browsers can't do
VPNs": a browser cannot open a raw UDP socket, so it cannot run *native* WireGuard.
It can, however, run a **WireGuard stack compiled to WebAssembly inside the tab**,
carrying its transport over WebSocket or WebTransport.

This is not theoretical. Tailscale ships exactly this as **`tsconnect`** — a WASM
build of its WireGuard client that lets a browser tab join a tailnet as a real node
with a real `100.x` address and genuine end-to-end WireGuard encryption. It is open
source and is the reference implementation to study.

What this does and does not buy us:

- **Does:** real WireGuard encryption and a stable mesh address for a friend who
  installs *nothing*. The browser reaches `10.66.0.1:8443` as if on a LAN.
- **Does not:** remove the reachability requirement. The WASM stack still needs some
  endpoint it can open a WebSocket/WebTransport connection to. If that endpoint is
  the host and the host is unreachable, the ladder below still applies.
  WebTransport (HTTP/3) does expose unreliable datagrams, which is the closest a
  browser gets to real UDP and the better long-term transport.

So the VPN and the NAT-traversal problems are **separate**. Solving "browser joins
the VPN" does not solve "something is reachable," and vice versa. Both need answers.

**And the browser path already works.** Evidence from a real session: ICE selected
`76.35.135.156` ↔ `73.152.148.83`, both `srflx`, `iceConnectionState connected`,
video flowing — a direct peer-to-peer connection through both NATs with no relay and
no port forward. The two failures that session were unrelated to connectivity:
bitrate (18 Mbps at 1080p60 over a cable uplink) and a stale RPCS3 pad binding.

So this plan has three tracks:

- **Track A — make the existing ICE path reliable** (helps every friend, no install,
  cheapest win)
- **Track B — direct WireGuard for native clients** (removes relays from the steady
  state)
- **Track C — WireGuard-in-WASM for browser friends** (a real VPN with zero install;
  study `tsconnect` first)

---

## The connectivity ladder

Try in order, use the first that verifies, fall back automatically. Nothing here
requires the user to configure a router.

| Tier | Path | Requires | Works when |
|------|------|----------|------------|
| 0 | Same LAN | nothing | both on one network |
| 1 | IPv6 direct | global IPv6 both ends + local firewall allow | ~45% of residential pairs today |
| 2 | IPv4 direct | a forwarded/UPnP-mapped port on **either** side | one side has cooperative gear |
| 3 | **IPv4 UDP hole punch** | STUN + a coordination channel | **most residential NAT pairs** |
| 4 | Relay | reachable TURN / DERP / tunnel | always — last resort |

Tier 3 is the one that makes this "work outside the box," and it is the tier the
codebase currently has no implementation of for WireGuard.

### Honest coverage limits

Hole punching is not magic. It succeeds for full-cone, restricted-cone, and
port-restricted-cone NATs — the large majority of home routers. It **fails when both
peers are behind symmetric NAT**, which allocates a fresh external port per
destination, so the port learned via STUN is not the port the peer must send to.
Carrier-grade NAT (common on mobile and some fibre/rural ISPs) behaves this way.

For that population Tier 4 is not a fallback, it is *the* answer. A design that
cannot relay is a design that abandons those users. **Tier 4 must remain permanently
supported, not treated as a temporary crutch.**

---

## Mechanism for Tier 3 (WireGuard through NAT, no third party in steady state)

WireGuard has no built-in NAT traversal. It is added around it:

1. **Discover** each side's public UDP endpoint (external IP:port) via STUN, using
   the *same local port* WireGuard will bind. Reuse the public STUN servers already
   configured for WebRTC.
2. **Exchange** the discovered endpoints over the existing couchlink signaling
   channel — the friend already has a working path to signaling, so no new
   infrastructure is needed. This is the coordination channel, exactly the role DERP
   plays for Tailscale.
3. **Simultaneous open:** both sides begin sending WireGuard handshake initiations to
   each other's discovered endpoint at once. Each outbound packet creates NAT state
   permitting the inbound reply.
4. **Keepalive:** `PersistentKeepalive = 25` on **both** peers (currently player-only)
   so the mapping does not expire.
5. **Verify, then commit:** only after `wg show <if> latest-handshakes` reports a
   recent non-zero handshake is the mesh considered up.

Bootstrap honesty: reaching signaling may itself require the tunnel/relay on the very
first connection. That is acceptable and is how Tailscale works — bootstrap over the
relay, then upgrade to direct and drop it. The steady-state path is direct; the relay
is only a rendezvous.

---

## Design principles (apply to every task)

- **Detect, never require.** No step may hard-depend on IPv6, UPnP, or a forward.
- **Verify before advertising.** Never print a join URL for a path not proven
  reachable. Finding #4 is the canonical violation.
- **Fail down, not out.** Every tier failure falls to the next tier automatically.
- **No silent success.** Every "OK" must correspond to an observed fact, not the
  absence of an error.

---

## Phase 1 — Stop advertising dead paths (do this first, standalone value)

This phase is worth shipping alone; it fixes a live bug that strands sessions.

- [ ] Add a handshake-liveness check to `couchlink_wireguard_ip`
      (`scripts/lib-mesh.sh:107-121`): require `wg show "$ifc" latest-handshakes` to
      report a peer with a non-zero, recent timestamp. An interface that exists but
      has never handshaked must report failure.
- [ ] Preserve the `COUCHLINK_WG_FORCE` escape hatch (lines 113-116) for the
      Windows-WireGuard / WSL-host case where `wg` is not visible from Linux —
      but log loudly that liveness was skipped.
- [ ] Add a generic `couchlink_verify_reachable` helper used before any join URL is
      printed, so Headscale and WireGuard share one gate.
- [ ] Correct the false "8443/3478" portproxy log in
      `scripts/windows/enable-wireguard.ps1:131-146` and the wrong UDP comments at
      `lib-mesh.sh:138,167` — `netsh portproxy` is TCP-only.

## Phase 2 — Endpoint flexibility (removes the IPv4-only assumption)

- [ ] Add `COUCHLINK_WG_ENDPOINT` to `scripts/setup-wireguard.sh` to override the
      endpoint host (bare IPv6, IPv4, or hostname). Do **not** overload
      `COUCHLINK_PUBLIC_IP` — it is already consumed by `run.sh:252-266` and
      `enable-headscale.sh:91`.
- [ ] Support IPv6 endpoints with correct bracket syntax
      (`Endpoint = [2603:...]:51820`); unbracketed will not parse.
- [ ] Prefer a global IPv6 **when detected**, else IPv4. Reuse the address written by
      `enable-upnp.ps1` to `public-ipv6.txt`, already read by
      `lib-online-tunnel.sh:15`.
- [ ] Emit a peer `Endpoint` in `wg0-host.conf` so the host can initiate — required
      for both the reverse-listener arrangement and Tier 3.
- [ ] Set `PersistentKeepalive = 25` on both peers, not just the player.
- [ ] Handle endpoint staleness: the baked-in address goes stale on any DHCP change.
      Re-resolve at bring-up rather than trusting generation time.

## Phase 3 — Tier 3 hole punching (the "works anywhere" tier)

- [ ] Add STUN-based endpoint discovery bound to the WireGuard port, reusing the STUN
      servers already configured for WebRTC.
- [ ] Extend the signaling protocol (`crates/proto/src/signal.rs`) with an
      endpoint-exchange message. Treat it as a wire-format change — see
      `superpowers:data-migration-safety`; old clients must not break.
- [ ] Implement simultaneous-open: both peers send handshake initiations on a short
      retry schedule until a handshake lands or a timeout expires.
- [ ] Detect symmetric NAT (STUN reports differing external ports per destination)
      and skip straight to Tier 4 rather than retrying a doomed punch.
- [ ] Enforce an overall timeout so a failed punch falls to Tier 4 quickly — a user
      waiting 30s for a doomed handshake is a worse outcome than an instant relay.

## Phase 4 — Track A: make the relay-free browser path reliable

Highest user value per unit of work, and it helps friends who install nothing.

- [ ] Make bitrate adaptive to measured uplink instead of fixed per preset.
      `crates/proto/src/signal.rs:96` requests 18 Mbps at 1080p60, which exceeds a
      typical cable upload and produced the observed `decodeFps` swings of 5→46.
- [ ] Ensure a reachable TURN for the symmetric-NAT population. Note the IPv4/IPv6
      asymmetry: a TURN URL on the host's IPv6 is useless to an IPv4-only friend, and
      vice versa. Advertise both families when available.
- [ ] Surface the selected ICE candidate pair in host logs, so "it's laggy" can be
      diagnosed as relay-vs-direct without reading browser console dumps.

## Phase 5 — Native client without an admin install (optional, "outside the box")

- [ ] Evaluate embedding userspace WireGuard (`boringtun` / `wireguard-go`) in
      `couchlink-client` so the friend needs no separate WireGuard app and no
      administrator rights. This is the difference between "install an app and import
      a config" and "run one binary" — historically the thing friends refuse.
- [ ] Compare against the current ask (WireGuard app + `.conf` import) and record the
      tradeoff in `docs/WIREGUARD.md`.

## Phase 5b — Track C: WireGuard-in-WASM for browser friends

The zero-install VPN. Larger than the other phases, so scope it deliberately.

- [ ] Study Tailscale's `tsconnect` (WASM WireGuard in a browser tab) as the
      reference implementation — how it drives the WireGuard state machine from
      WASM, and how it carries transport when raw UDP is unavailable.
- [ ] Choose a transport: **WebTransport** (HTTP/3, exposes unreliable datagrams —
      the closest browsers get to UDP, best latency for game streaming) with a
      **WebSocket** fallback for browsers or networks where HTTP/3 is blocked.
- [ ] Evaluate compiling `wireguard-go` or `boringtun` to `wasm32-unknown-unknown`
      and driving it from the existing web client in `web/`.
- [ ] Decide what the browser peer terminates against: the host directly when
      reachable, otherwise a relay. This is where Track C meets the ladder — the
      WASM peer still needs *some* reachable endpoint.
- [ ] Measure the added latency versus today's WebRTC path before adopting it.
      WebRTC's media stack is heavily optimised for realtime; tunnelling video
      through a WASM WireGuard session may cost more than it gains. **If it is
      slower than plain WebRTC ICE, it is not worth shipping for video** — but may
      still be worth it for a stable mesh address and uniform addressing.

## Phase 6 — Documentation

- [ ] Rewrite `docs/WIREGUARD.md` around the ladder: what each tier needs, how to
      tell which one is active, and what to do when stuck at Tier 4.
- [ ] Remove the unimplemented "relay/VPS Endpoint" claim (line 24).
- [ ] Document every new env var in one table.
- [ ] Add a troubleshooting section keyed to observable symptoms, not causes
      (`docs/NETWORK_SPECTRUM.md` is the model — it records the gateway's read-only
      admin page and disabled UPnP so the next reader does not rediscover them).

---

## Files

| File | Change |
|---|---|
| `scripts/lib-mesh.sh` | Handshake liveness; shared reachability gate; fix wrong UDP comments |
| `scripts/setup-wireguard.sh` | `COUCHLINK_WG_ENDPOINT`, IPv6 brackets, peer endpoint, both-side keepalive |
| `scripts/enable-wireguard.sh` | Re-resolve endpoint at bring-up; report liveness honestly |
| `scripts/windows/enable-wireguard.ps1` | Correct portproxy log; UDP 51820 firewall |
| `scripts/run.sh` | Ladder ordering; never advertise an unverified path |
| `crates/proto/src/signal.rs` | Endpoint-exchange message; adaptive bitrate |
| `docs/WIREGUARD.md` | Ladder-based rewrite |

---

## Verification

A matrix, because "works on my machine" is exactly the failure mode here. Each tier
must be tested with the tiers above it disabled.

| Scenario | Expected tier | Pass condition |
|----------|---------------|----------------|
| Both on one LAN | 0 | direct, no STUN |
| Both IPv6, IPv4 blocked | 1 | handshake over IPv6 |
| Host forwards 51820 | 2 | player→host direct |
| Both NATed, no forward, no IPv6 | 3 | handshake after punch |
| Symmetric NAT both ends | 4 | fast fall to relay, no long stall |
| Tunnel up but handshake dead | 4 | **does not** advertise `10.66.0.x` (Phase 1 regression test) |

- [ ] `wg show wg0 latest-handshakes` shows a recent non-zero timestamp on **both**
      ends — the only real proof; "interface is up" is not.
- [ ] `ping6`/`ping` across the tunnel, then `curl http://10.66.0.1:8443/` from the
      friend.
- [ ] Kill the tunnel mid-session; confirm automatic fall to the next tier.
- [ ] Full session: video connects and RPCS3 P2 input registers.
- [ ] `cargo test --workspace` stays green (currently 14 suites).

---

## Risks and open questions

- **Symmetric NAT is unsolvable directly.** Tier 4 must stay first-class. Any claim
  of "no relay ever" is false for that population.
- **Bootstrap dependency.** First contact may need the relay to exchange endpoints.
  Acceptable, but it means "no third party" is true of the steady state, not of
  session setup.
- **Zero-install is achievable, but Track C is the expensive route to it.** A browser
  friend *can* join the VPN via WASM WireGuard (Phase 5b), so "install nothing" and
  "real VPN" are not in conflict. But Track A gets a working, low-latency session for
  far less work — do not let Track C block it.
- **Spectrum IPv6 firewall unverified.** Whether this gateway permits inbound IPv6 is
  untested. Test before relying on Tier 1 — its admin page is read-only, so if it
  blocks inbound IPv6 there may be no way to change it.
- **Scope.** Phases 1 and 4 deliver most of the real-world benefit. Phase 3 is the
  largest engineering effort and should not block them.
