# Network setup — Spectrum (Askey MAX2V1K)

Notes for hosting couchlink behind a Spectrum-issued gateway. Everything here was
verified against this network on 2026-08-02.

## What this gateway is

| | |
|---|---|
| Model | Askey **MAX2V1K** ("Spectrum Advanced WiFi Router") |
| Local admin | `https://192.168.1.1` — **read-only** |
| Config path | My Spectrum app only (`https://www.spectrum.net/getapp`) |

The local page at `192.168.1.1` serves Model, Serial, Internet Status and a single
link to the My Spectrum app. There is **no local port-forwarding page and no UPnP
toggle**. Confirmed side effect: `HNetCfg.NATUPnP` returns a null
`StaticPortMappingCollection`, so `enable-upnp.ps1` cannot auto-map anything.
All router changes must go through the app.

## Addresses

| Thing | Value |
|---|---|
| Windows LAN IP (Wi-Fi) | `192.168.1.223` — forward to this |
| WSL IP | `172.18.223.133` |
| Public IPv4 | `76.35.135.156` |
| Public IPv6 | `2603:6011:10f0:9ce0::1dbd` |

LAN and WSL IPs can change after a reboot or DHCP lease change. Re-check with
`ip -4 addr show eth0` (WSL) before trusting a stale forward.

## Ports

| Port | Proto | Purpose | Needed when |
|---|---|---|---|
| 8080 | TCP | Headscale control plane | Mesh join over public IPv4 |
| 8443 | TCP | Signaling / web client | Direct (non-mesh) online path |
| 3478 | TCP+UDP | TURN relay | Direct path, when P2P fails |
| 34790 | UDP | Embedded DERP STUN | Only if embedded DERP is enabled |

For the mesh path, **TCP 8080 is the only forward required**. The data plane relays
through Tailscale's public DERP servers, which is outbound-only.

`netsh portproxy` is TCP-only, so the UDP entries (3478, 34790) are covered by the
firewall rules and direct binds, not by portproxy.

## Already done on this machine

These persist across reboots — no need to redo them:

- Windows firewall inbound allows: `couchlink-headscale-control-8080` (TCP 8080),
  `couchlink-headscale-stun-34790` (UDP 34790), plus the 8443/3478 rules.
- `netsh` portproxy for 8080, both families, into WSL:

  ```
  0.0.0.0  8080  ->  172.18.223.133  8080
  ::       8080  ->  172.18.223.133  8080
  ```

Verified working end to end from Windows:

```powershell
Invoke-WebRequest http://192.168.1.223:8080/health   # 200
```

So everything from "packet reaches the router" onward already works.

## What you still have to do

Open the **My Spectrum app** → sign in → **Services → Internet →** your router →
**Advanced Settings**. Then either:

1. **Enable UPnP** (preferred). One-time. After this, `enable-upnp.ps1` maps 8080
   automatically on every `--online` run and you never touch the router again.
2. **Add a port forward**: TCP **8080** → **192.168.1.223**.

If Advanced Settings is not exposed on your firmware version, call Spectrum and ask
them to either enable UPnP or put the gateway in **bridge mode**. Bridge mode plus
your own router gives you permanent control and is the durable fix.

## Verifying

External reachability (run from anywhere):

```bash
curl -sS -X POST https://portchecker.io/api/v1/query \
  -H 'Content-Type: application/json' \
  -d '{"host":"76.35.135.156","ports":[8080]}'
```

`"status": true` means the forward is live. While it reads `false`, a friend's client
cannot register with Headscale at all, and the `100.64.0.2` mesh IP in a join URL is
dead on arrival — that is the failure mode this document exists to prevent.

## Paths that avoid the router entirely

- **IPv6** — no NAT, so no forward needed. Requires the *friend* to have IPv6; check
  at `test-ipv6.com`. Set `COUCHLINK_HS_URL=http://[2603:6011:10f0:9ce0::1dbd]:8080`.
  The v6 portproxy is already in place.
- **Headscale on a VPS** — a public IP removes this gateway from the problem
  permanently. Your machine becomes an ordinary node instead of the server.
- **cloudflared** (`COUCHLINK_HS_USE_CLOUDFLARED=1`) — outbound-only, works behind
  any NAT. Caveat: see `scripts/enable-headscale.sh:75`. Measured through a quick
  tunnel, `/key?v=115` returns 200 but the Noise endpoint `/ts2021` returns 500
  versus 400 locally, so control-plane registration may not survive the tunnel.

## See also

- `docs/HEADSCALE.md` — mesh setup and join flow
- `scripts/enable-upnp.sh` — firewall + portproxy + UPnP mapping
- `scripts/install-windows-helper.sh` — run from a local Windows path, not
  `\\wsl.localhost\...`; elevated processes reject UNC paths
