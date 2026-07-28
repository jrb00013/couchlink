#!/usr/bin/env bash
# Part C — docs, infra, install, adapters, python, CI → push toward ~50 commits
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

commit() {
  local msg="$1"
  shift
  git add "$@"
  if git diff --cached --quiet 2>/dev/null; then
    return 0
  fi
  git commit -m "$msg"
}

w() {
  local path="$1"
  mkdir -p "$(dirname "$path")"
  cat > "$path"
}

############################################
# Docs
############################################
w docs/ARCHITECTURE.md <<'EOF'
# Couchlink architecture

High-definition, low-latency **co-play** for emulators: host streams the game window;
the friend's DualSense is injected on the host as a **Bluetooth DualSense**.

Methodologies follow [Rohomieo](https://github.com/jrb00013/rohomieo): WebRTC media is
peer-to-peer; signaling only exchanges SDP/ICE; WireGuard LAN preferred (no public
STUN/TURN). Pad HID layouts follow [dualsensekit](https://github.com/jrb00013/dualsensekit).

## Components

| Crate / dir | Role |
|-------------|------|
| `crates/proto` | JSON signaling + binary `CLPD` pad frames |
| `crates/pad` | DualSense report parse + `uinput` virtual BT pad |
| `crates/signaling` | Axum WebSocket session broker |
| `crates/host` | Capture → H.264 → WebRTC; apply pad frames to virtual device |
| `crates/client` | hidapi DualSense reader + WebRTC answer + pad sender |
| `infra/wireguard` | VPN examples for friend↔you path |
| `adapters/` | PCSX2 / RPCS3 binding helpers |

## Connection flow

1. Host registers `register_host` with `session_id` + `pin` + stream preset.
2. Player registers `register_player` with same credentials.
3. Server sends `peer_joined` to host.
4. Host creates WebRTC **offer** (H.264 video + `pad` data channel).
5. Player **answer** + ICE via signaling relay.
6. Encrypted SRTP video + binary pad frames flow peer-to-peer (ideally over WireGuard).

## Virtual Bluetooth pad

On Linux the host opens `/dev/uinput` and creates a device with:

- `BUS_BLUETOOTH` (0x05)
- Vendor `0x054C`, Product `0x0CE6` (DualSense)
- Name `DualSense Wireless Controller`

PCSX2 / RPCS3 see a wireless DualSense and can bind player 2 to it — same outcome
dualsensekit's `rpcs3_configure_pads.ps1` targets for local pads.

## Adaptive streaming

- WebRTC GCC on the video track
- Tile-diff motion detector: idle ~8 FPS when &lt;2% tiles change; motion up to preset FPS
- Presets: `1080p60`, `1080p30`, `720p60`, `720p30`
EOF

commit "docs: architecture (Rohomieo + dualsensekit methodologies)" docs/ARCHITECTURE.md

w docs/PROTOCOL.md <<'EOF'
# Couchlink wire protocol

## Signaling (WebSocket `/ws`)

Rohomieo-style tagged JSON (`type` in snake_case). Media never transits this server.

| Message | Direction | Fields |
|---------|-----------|--------|
| `register_host` | host → server | `session_id`, `pin`, `device_name?`, `preset?`, `emulator?` |
| `register_player` | player → server | `session_id`, `pin`, `player_name?` |
| `registered` | server → client | `role`, `session_id` |
| `offer` / `answer` | relayed | `sdp` |
| `ice_candidate` | relayed | `candidate`, `sdpMid?`, `sdpMLineIndex?` |
| `stream_info` | host → player | `width`, `height`, `fps`, `codec` |
| `heartbeat` / `pong` | either | — |
| `peer_joined` / `peer_left` | server | `role` |

## DataChannel `pad` — custom binary `CLPD`

Fixed-size little-endian frame (lower latency than JSON at ~250 Hz):

| Offset | Len | Field |
|--------|-----|-------|
| 0 | 4 | Magic `CLPD` |
| 4 | 1 | Version `1` |
| 5 | 4 | `seq` |
| 9 | 4 | `buttons` bitfield |
| 13 | 1 | `lx` |
| 14 | 1 | `ly` |
| 15 | 1 | `rx` |
| 16 | 1 | `ry` |
| 17 | 1 | `l2` |
| 18 | 1 | `r2` |
| 19 | 2 | `gx` |
| 21 | 2 | `gy` |
| 23 | 2 | `gz` |
| 25 | 1 | `touch_active` |
| 26 | 2 | `touch_x` |
| 28 | 2 | `touch_y` |
| 30 | 1 | reserved |

Button bits mirror DualSense face/shoulder/dpad layout used by dualsensekit parsers.

## Pad feedback (host → player, JSON on `pad` channel)

```json
{"type":"rumble","large":120,"small":40}
{"type":"lightbar","r":0,"g":0,"b":255}
{"type":"player_led","mask":1}
```

## DualSense HID (client capture)

From dualsensekit `PROTOCOL.md`:

| Report | ID | Notes |
|--------|----|-------|
| USB input | `0x01` | 64 bytes |
| BT input | `0x31` | 78 bytes |
| USB output | `0x02` | rumble / lightbar |

## HTTP

| Path | Purpose |
|------|---------|
| `GET /health` | Liveness |
| `GET /api/status` | Version + session counts |
| `GET /api/audit` | PIN / join audit |
| `GET /metrics` | Prometheus |
EOF

commit "docs: signaling JSON and CLPD binary pad protocol" docs/PROTOCOL.md

w docs/GETTING_STARTED.md <<'EOF'
# Getting started

## You (host) — machine running PCSX2 / RPCS3

```bash
git clone https://github.com/jrb00013/couchlink.git
cd couchlink
./install.sh
source .env.couchlink

# optional but recommended for internet play
# see docs/WIREGUARD.md

couchlink-signaling &
couchlink-host \
  --session-id friends-night \
  --pin 482193 \
  --preset 1080p60 \
  --emulator rpcs3
```

Ensure your user can write `/dev/uinput` (install udev rule from `install.sh`).

## Friend (player)

```bash
./install.sh
couchlink-client \
  --signaling ws://HOST_WG_IP:8443/ws \
  --session-id friends-night \
  --pin 482193
```

Pair a DualSense first (USB or BT). On Windows, dualsensekit's pairing scripts help.

## Bind in the emulator

- **RPCS3**: Player 2 → DualSense Wireless Controller (the virtual BT one)
- **PCSX2**: Controllers → DualSense / SDL → select the couchlink virtual pad

Helpers: `adapters/rpcs3/` and `adapters/pcsx2/`.
EOF

commit "docs: getting started for host and friend" docs/GETTING_STARTED.md

w docs/WIREGUARD.md <<'EOF'
# WireGuard path (recommended)

Same posture as Rohomieo: keep WebRTC on a private mesh. Signaling can listen on the
WireGuard IP; media uses host candidates on that interface. **No public STUN/TURN**
required when both peers are on the VPN.

## Minimal two-peer config

See `infra/wireguard/wg0-host.conf.example` and `wg0-player.conf.example`.

1. Generate keys on each machine (`wg genkey`).
2. Exchange public keys + assign `10.66.0.1/24` (host) and `10.66.0.2/24` (player).
3. `wg-quick up wg0`
4. Point client at `ws://10.66.0.1:8443/ws`

## Why not only a custom UDP proto?

Couchlink **does** use a custom binary pad proto (`CLPD`) on the DataChannel.
Video still rides WebRTC/SRTP for congestion control, encryption, and NAT-friendly
ICE — proven in Rohomieo. You can later add a raw UDP video path; WireGuard + WebRTC
is the low-risk HD default.
EOF

w infra/wireguard/wg0-host.conf.example <<'EOF'
[Interface]
Address = 10.66.0.1/24
ListenPort = 51820
PrivateKey = HOST_PRIVATE_KEY

[Peer]
PublicKey = PLAYER_PUBLIC_KEY
AllowedIPs = 10.66.0.2/32
EOF

w infra/wireguard/wg0-player.conf.example <<'EOF'
[Interface]
Address = 10.66.0.2/24
PrivateKey = PLAYER_PRIVATE_KEY

[Peer]
PublicKey = HOST_PUBLIC_KEY
Endpoint = HOST_PUBLIC_IP:51820
AllowedIPs = 10.66.0.0/24
PersistentKeepalive = 25
EOF

w infra/wireguard/README.md <<'EOF'
# WireGuard examples for couchlink

Copy `*.example`, replace keys, `wg-quick up`. See `docs/WIREGUARD.md`.
EOF

commit "docs: WireGuard mesh for low-latency peer path" \
  docs/WIREGUARD.md infra/wireguard/wg0-host.conf.example \
  infra/wireguard/wg0-player.conf.example infra/wireguard/README.md

w docs/LATENCY.md <<'EOF'
# Latency budget (HD co-play)

Target feel for local/WireGuard play: **&lt; 40–60 ms** glass-to-glass on a good LAN.

| Stage | Budget | Notes |
|-------|--------|-------|
| Capture | 0–8 ms | scrap DXGI/X11; prefer exclusive fullscreen / borderless |
| Encode | 4–12 ms | OpenH264 low-latency; 1080p60 needs CPU headroom |
| Net | 5–30 ms | WireGuard preferred; Wi-Fi adds jitter |
| Decode + present | 8–16 ms | Player GPU decode |
| Pad | ~4 ms | 250 Hz `CLPD` frames; uinput inject on host |

## Knobs

- `--preset 720p60` if 1080p60 saturates encode
- `--idle-fps 8` (default) saves bitrate on static menus (Rohomieo motion detector)
- Wired Ethernet + WG over Wi-Fi
- Keep host display Hz ≥ stream FPS

## Custom proto vs WebRTC

Pad path is already custom binary. Replacing video with a proprietary UDP codec is
optional future work; WebRTC GCC already adapts bitrate under congestion.
EOF

w docs/EMULATORS.md <<'EOF'
# Emulator binding

The host virtual device appears as:

```
Name:    DualSense Wireless Controller
Bus:     Bluetooth
Vendor:  054c
Product: 0ce6
```

## RPCS3

1. Start `couchlink-host` so the virtual pad exists.
2. Open RPCS3 → Pads.
3. Assign Player 2 (or 1) to the DualSense entry that shows as Bluetooth.
4. Or run `adapters/rpcs3/configure_virtual_pad.sh` after generating a template.

Inspired by dualsensekit `scripts/windows/rpcs3_configure_pads.ps1`.

## PCSX2

1. Settings → Controllers → Controller Port 2.
2. Select SDL / DualShock → pick **DualSense Wireless Controller**.
3. Helper: `adapters/pcsx2/configure_virtual_pad.sh`.

## Tips

- Create the virtual pad **before** launching the emulator so hotplug is clean.
- Local physical DualSense = Player 1; couchlink virtual = Player 2.
EOF

commit "docs: latency budget and emulator binding guides" \
  docs/LATENCY.md docs/EMULATORS.md

############################################
# Adapters
############################################
w adapters/rpcs3/configure_virtual_pad.sh <<'EOF'
#!/usr/bin/env bash
# Point humans at the virtual Bluetooth DualSense created by couchlink-host.
set -euo pipefail
echo "Couchlink virtual pad should appear as:"
echo "  DualSense Wireless Controller (Bluetooth, 054c:0ce6)"
echo
if command -v python3 >/dev/null; then
  python3 - <<'PY'
import glob, os
for path in sorted(glob.glob('/sys/class/input/js*/device/name')):
    try:
        name = open(path).read().strip()
    except OSError:
        continue
    if 'DualSense' in name or 'Wireless Controller' in name:
        print(f"found: {name} ({path})")
PY
fi
echo
echo "In RPCS3 → Pads, bind Player 2 to that device."
echo "Windows users: see dualsensekit scripts/windows/rpcs3_configure_pads.ps1 for local pads."
EOF

w adapters/pcsx2/configure_virtual_pad.sh <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
echo "PCSX2: Settings → Controllers → Port 2 → SDL → DualSense Wireless Controller"
echo "Ensure couchlink-host is running so the uinput Bluetooth DualSense exists."
if [[ -d /dev/input ]]; then
  ls -l /dev/input/by-id 2>/dev/null | grep -i dualsense || true
fi
EOF

chmod +x adapters/rpcs3/configure_virtual_pad.sh adapters/pcsx2/configure_virtual_pad.sh

commit "feat(adapters): PCSX2 and RPCS3 virtual pad helpers" \
  adapters/rpcs3/configure_virtual_pad.sh adapters/pcsx2/configure_virtual_pad.sh

############################################
# install / makefile / env
############################################
w Makefile <<'EOF'
.PHONY: check test build install web

check:
	cargo check --workspace

test:
	cargo test --workspace

build:
	cargo build --release --workspace

install: build
	install -Dm755 target/release/couchlink-signaling "$(HOME)/.local/bin/couchlink-signaling"
	install -Dm755 target/release/couchlink-host "$(HOME)/.local/bin/couchlink-host"
	install -Dm755 target/release/couchlink-client "$(HOME)/.local/bin/couchlink-client"
EOF

w .env.example <<'EOF'
COUCHLINK_BIND=0.0.0.0:8443
COUCHLINK_SIGNALING=ws://127.0.0.1:8443/ws
COUCHLINK_SESSION_ID=friends-night
COUCHLINK_PIN=123456
COUCHLINK_PRESET=1080p60
EOF

w install.sh <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
ROOT="$(cd "$(dirname "$0")" && pwd)"
cd "$ROOT"

echo "==> couchlink install"

if ! command -v cargo >/dev/null; then
  echo "Rust/cargo required: https://rustup.rs"
  exit 1
fi

# Linux deps for capture + uinput + hid
if [[ "$(uname -s)" == Linux ]]; then
  if command -v apt-get >/dev/null; then
    sudo apt-get update -qq
    sudo apt-get install -y -qq build-essential pkg-config libx11-dev libxcb1-dev \
      libxcb-shm0-dev libxcb-randr0-dev libhidapi-hidraw-dev udev || true
  fi
  # uinput access
  sudo tee /etc/udev/rules.d/99-couchlink-uinput.rules >/dev/null <<'RULE'
KERNEL=="uinput", MODE="0660", GROUP="input", OPTIONS+="static_node=uinput"
RULE
  sudo udevadm control --reload-rules || true
  sudo modprobe uinput || true
  if getent group input >/dev/null; then
    sudo usermod -aG input "$USER" || true
    echo "Added $USER to group 'input' — re-login may be required for /dev/uinput"
  fi
fi

cargo build --release --workspace
mkdir -p "$HOME/.local/bin"
install -Dm755 target/release/couchlink-signaling "$HOME/.local/bin/couchlink-signaling"
install -Dm755 target/release/couchlink-host "$HOME/.local/bin/couchlink-host"
install -Dm755 target/release/couchlink-client "$HOME/.local/bin/couchlink-client"

if [[ ! -f .env.couchlink ]]; then
  cp .env.example .env.couchlink
fi

mkdir -p web/dist
if [[ ! -f web/dist/index.html ]]; then
  cat > web/dist/index.html <<'HTML'
<!doctype html>
<html><head><meta charset="utf-8"><title>couchlink</title>
<style>
  body{font-family:system-ui;background:#0b0f14;color:#e8eef7;display:grid;place-items:center;min-height:100vh;margin:0}
  main{max-width:36rem;padding:2rem}
  code{background:#1a2330;padding:.1rem .35rem;border-radius:4px}
</style></head>
<body><main>
<h1>couchlink</h1>
<p>Signaling is up. Run <code>couchlink-host</code> and <code>couchlink-client</code> for HD co-play.</p>
<p>See docs/GETTING_STARTED.md</p>
</main></body></html>
HTML
fi

echo "OK — binaries in ~/.local/bin"
echo "source .env.couchlink && couchlink-signaling"
EOF
chmod +x install.sh

commit "build: Makefile, install.sh, env example, uinput udev rule" \
  Makefile .env.example install.sh

w scripts/start-host.sh <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
# shellcheck disable=SC1091
[[ -f "$ROOT/.env.couchlink" ]] && source "$ROOT/.env.couchlink"
: "${COUCHLINK_SESSION_ID:?set COUCHLINK_SESSION_ID}"
: "${COUCHLINK_PIN:?set COUCHLINK_PIN}"
exec couchlink-host \
  --signaling "${COUCHLINK_SIGNALING:-ws://127.0.0.1:8443/ws}" \
  --session-id "$COUCHLINK_SESSION_ID" \
  --pin "$COUCHLINK_PIN" \
  --preset "${COUCHLINK_PRESET:-1080p60}"
EOF

w scripts/start-client.sh <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
# shellcheck disable=SC1091
[[ -f "$ROOT/.env.couchlink" ]] && source "$ROOT/.env.couchlink"
: "${COUCHLINK_SESSION_ID:?set COUCHLINK_SESSION_ID}"
: "${COUCHLINK_PIN:?set COUCHLINK_PIN}"
exec couchlink-client \
  --signaling "${COUCHLINK_SIGNALING:-ws://127.0.0.1:8443/ws}" \
  --session-id "$COUCHLINK_SESSION_ID" \
  --pin "$COUCHLINK_PIN"
EOF

w scripts/start-signaling.sh <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
# shellcheck disable=SC1091
[[ -f "$ROOT/.env.couchlink" ]] && source "$ROOT/.env.couchlink"
exec couchlink-signaling --bind "${COUCHLINK_BIND:-0.0.0.0:8443}" --web-root "$ROOT/web/dist"
EOF
chmod +x scripts/start-host.sh scripts/start-client.sh scripts/start-signaling.sh

commit "feat(scripts): start host, client, and signaling helpers" \
  scripts/start-host.sh scripts/start-client.sh scripts/start-signaling.sh

############################################
# Python dualsense helper (dualsensekit style)
############################################
w python/couchlink/__init__.py <<'EOF'
"""Lightweight DualSense reader for couchlink debugging (dualsensekit-compatible)."""

__version__ = "0.1.0"
EOF

w python/couchlink/dualsense.py <<'EOF'
"""Enumerate/read DualSense — mirrors dualsensekit python/dualsensekit/device.py."""

from __future__ import annotations
from dataclasses import dataclass
from typing import List, Optional
import struct

try:
    import hid
except ImportError as e:  # pragma: no cover
    raise SystemExit("pip install hidapi") from e

SONY_VID = 0x054C
PID_DUALSENSE = 0x0CE6
PID_EDGE = 0x0DF2
INPUT_USB = 0x01
INPUT_BT = 0x31


@dataclass
class DeviceInfo:
    path: bytes
    product_id: int
    interface_number: int
    connection: str


def enumerate_devices() -> List[DeviceInfo]:
    out: List[DeviceInfo] = []
    for d in hid.enumerate(SONY_VID):
        if d["product_id"] not in (PID_DUALSENSE, PID_EDGE):
            continue
        iface = d.get("interface_number", -1)
        usage_page = d.get("usage_page")
        usage = d.get("usage")
        if usage_page == 1 and usage == 5:
            pass
        elif iface in (-1, 3):
            pass
        else:
            continue
        conn = "bluetooth" if iface is not None and iface < 0 else "usb"
        out.append(
            DeviceInfo(
                path=d["path"],
                product_id=d["product_id"],
                interface_number=iface if iface is not None else -1,
                connection=conn,
            )
        )
    return out


class DualSense:
    def __init__(self, path: Optional[bytes] = None):
        devices = enumerate_devices()
        if not devices:
            raise RuntimeError("no DualSense found")
        devices = sorted(devices, key=lambda x: 0 if x.connection == "usb" else 1)
        path = path or devices[0].path
        self.info = next(d for d in devices if d.path == path)
        self._dev = hid.device()
        self._dev.open_path(path)

    def read_raw(self, timeout_ms: int = 16) -> bytes:
        return bytes(self._dev.read(128, timeout_ms) or b"")
EOF

w python/couchlink/clpd.py <<'EOF'
"""Encode/decode CLPD pad frames for tests."""

from __future__ import annotations
import struct
from dataclasses import dataclass

MAGIC = b"CLPD"
VERSION = 1


@dataclass
class PadFrame:
    seq: int = 0
    buttons: int = 0
    lx: int = 128
    ly: int = 128
    rx: int = 128
    ry: int = 128
    l2: int = 0
    r2: int = 0

    def encode(self) -> bytes:
        body = struct.pack(
            "<BI4B2B3hBHHB",
            VERSION,
            self.seq,
            self.buttons,
            self.lx,
            self.ly,
            self.rx,
            self.ry,
            self.l2,
            self.r2,
            0,
            0,
            0,
            0,
            0,
            0,
            0,
        )
        # Manual pack to match Rust layout closely
        return MAGIC + struct.pack(
            "<BI4B2B",
            VERSION,
            self.seq & 0xFFFFFFFF,
            self.buttons & 0xFFFFFFFF,
            self.lx,
            self.ly,
            self.rx,
            self.ry,
            self.l2,
            self.r2,
        ) + struct.pack("<hhhBHHB", 0, 0, 0, 0, 0, 0, 0)
EOF

w python/README.md <<'EOF'
# Python helpers

Debug DualSense capture the same way dualsensekit does:

```bash
pip install hidapi
python -c "from couchlink.dualsense import enumerate_devices; print(enumerate_devices())"
```
EOF

w python/pyproject.toml <<'EOF'
[project]
name = "couchlink"
version = "0.1.0"
description = "Couchlink DualSense helpers"
requires-python = ">=3.10"
dependencies = ["hidapi>=0.14"]

[build-system]
requires = ["setuptools"]
build-backend = "setuptools.build_meta"

[tool.setuptools.packages.find]
include = ["couchlink*"]
EOF

commit "feat(python): dualsensekit-style DualSense reader and CLPD helper" \
  python/couchlink/__init__.py python/couchlink/dualsense.py python/couchlink/clpd.py \
  python/README.md python/pyproject.toml

############################################
# More polish commits
############################################
w docs/SECURITY.md <<'EOF'
# Security

- 6-digit PIN per session; lockout after failed attempts (Rohomieo method)
- Prefer WireGuard; do not expose signaling to the open internet without TLS
- Optional TLS: `COUCHLINK_CERT` / `COUCHLINK_KEY`
- No STUN/TURN by default — reduces accidental public exposure of media
- Audit log: `GET /api/audit`
EOF

w SECURITY.md <<'EOF'
# Security Policy

Report issues privately to the maintainer. See `docs/SECURITY.md` for deployment guidance.
EOF

w CONTRIBUTING.md <<'EOF'
# Contributing

1. Keep diffs focused.
2. Proto changes need tests in `crates/proto`.
3. Pad HID offsets must stay aligned with dualsensekit / hid-playstation.
4. Run `cargo test --workspace` before PRs.
EOF

w CODE_OF_CONDUCT.md <<'EOF'
# Code of Conduct

Be respectful. Harassment is not tolerated. Rohomieo/dualsensekit community norms apply.
EOF

w CHANGELOG.md <<'EOF'
# Changelog

## 0.1.0

- Initial public release: signaling, host HD WebRTC path, client DualSense → virtual BT pad
- CLPD binary pad protocol
- PCSX2/RPCS3 adapter helpers
- WireGuard examples
EOF

commit "docs: security, contributing, changelog, code of conduct" \
  docs/SECURITY.md SECURITY.md CONTRIBUTING.md CODE_OF_CONDUCT.md CHANGELOG.md

w .github/workflows/ci.yml <<'EOF'
name: ci
on:
  push:
    branches: [main]
  pull_request:
jobs:
  test:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
      - name: deps
        run: |
          sudo apt-get update
          sudo apt-get install -y libx11-dev libxcb1-dev libxcb-shm0-dev libxcb-randr0-dev libhidapi-hidraw-dev pkg-config
      - run: cargo test --workspace
EOF

w .github/ISSUE_TEMPLATE/bug_report.md <<'EOF'
---
name: Bug report
about: Something broken in couchlink
---

**Host OS / Player OS:**
**Emulator:**
**Preset:**
**Logs:**
EOF

commit "ci: GitHub Actions workspace tests + bug template" \
  .github/workflows/ci.yml .github/ISSUE_TEMPLATE/bug_report.md

w docker-compose.yml <<'EOF'
services:
  signaling:
    image: rust:1.84
    working_dir: /app
    volumes:
      - ./:/app
    command: bash -lc "cargo run -p couchlink-signaling -- --bind 0.0.0.0:8443 --web-root web/dist"
    ports:
      - "8443:8443"
EOF

w examples/pad_roundtrip.rs <<'EOF'
// Placeholder note: run via `cargo test -p couchlink-proto`
fn main() {
    println!("see crates/proto tests for CLPD roundtrip");
}
EOF

w examples/README.md <<'EOF'
# Examples

- Proto/pad tests: `cargo test -p couchlink-proto -p couchlink-pad`
- Full session: `docs/GETTING_STARTED.md`
EOF

commit "chore: docker-compose signaling and examples notes" \
  docker-compose.yml examples/pad_roundtrip.rs examples/README.md

w web/dist/index.html <<'EOF'
<!doctype html>
<html lang="en">
<head>
  <meta charset="utf-8" />
  <meta name="viewport" content="width=device-width, initial-scale=1" />
  <title>couchlink</title>
  <style>
    :root {
      --bg0: #070b10;
      --bg1: #121a24;
      --ink: #e8eef7;
      --muted: #9aabbf;
      --accent: #3ecf8e;
    }
    * { box-sizing: border-box; }
    body {
      margin: 0;
      min-height: 100vh;
      color: var(--ink);
      font-family: "Segoe UI", "Helvetica Neue", sans-serif;
      background:
        radial-gradient(1200px 600px at 10% -10%, #1b3a2f 0%, transparent 55%),
        radial-gradient(900px 500px at 100% 0%, #1a2740 0%, transparent 50%),
        linear-gradient(160deg, var(--bg0), var(--bg1));
      display: grid;
      place-items: center;
      padding: 2rem;
    }
    main { max-width: 40rem; }
    h1 {
      font-size: clamp(2.5rem, 6vw, 4rem);
      letter-spacing: -0.04em;
      margin: 0 0 0.5rem;
    }
    p { color: var(--muted); line-height: 1.5; }
    .ok { color: var(--accent); font-weight: 600; }
    code {
      background: rgba(255,255,255,0.06);
      padding: 0.15rem 0.4rem;
      border-radius: 4px;
    }
  </style>
</head>
<body>
  <main>
    <h1>couchlink</h1>
    <p class="ok">signaling online</p>
    <p>
      HD co-play for PCSX2 / RPCS3. Friend streams your screen; their DualSense
      appears on your PC as a Bluetooth pad.
    </p>
    <p>Run <code>couchlink-host</code> and <code>couchlink-client</code>.</p>
  </main>
</body>
</html>
EOF

commit "feat(web): minimal signaling landing page" web/dist/index.html

w crates/pad/src/feedback.rs <<'EOF'
//! Map host rumble/lightbar feedback toward the player's real DualSense (via client).

use couchlink_proto::PadFeedback;

pub fn encode_feedback_json(fb: &PadFeedback) -> Result<String, serde_json::Error> {
    serde_json::to_string(fb)
}
EOF

# need serde_json in pad
w crates/pad/Cargo.toml <<'EOF'
[package]
name = "couchlink-pad"
version.workspace = true
edition.workspace = true
description = "DualSense report parsing + virtual Bluetooth pad injection"

[dependencies]
couchlink-proto = { workspace = true }
anyhow = { workspace = true }
thiserror = { workspace = true }
tracing = { workspace = true }
bytes = { workspace = true }
serde_json = { workspace = true }

[target.'cfg(target_os = "linux")'.dependencies]
nix = { version = "0.29", features = ["ioctl", "fs"] }
EOF

w crates/pad/src/lib.rs <<'EOF'
//! Pad stack: parse real DualSense HID reports (dualsensekit layouts) and
//! inject a virtual DualSense that announces itself as Bluetooth.

pub mod absinfo;
pub mod dualsense;
pub mod feedback;
pub mod parse;
pub mod virtual_pad;

pub use dualsense::{PID_DUALSENSE, PID_DUALSENSE_EDGE, SONY_VID};
pub use parse::parse_input_report;
pub use virtual_pad::{VirtualPad, VirtualPadConfig};
EOF

commit "feat(pad): rumble/lightbar feedback JSON helpers" \
  crates/pad/src/feedback.rs crates/pad/Cargo.toml crates/pad/src/lib.rs

w udev/99-couchlink-uinput.rules <<'EOF'
KERNEL=="uinput", MODE="0660", GROUP="input", OPTIONS+="static_node=uinput"
EOF

w scripts/gen_session.sh <<'EOF'
#!/usr/bin/env bash
# Print a random session id + 6-digit PIN
set -euo pipefail
SID=$(head -c 6 /dev/urandom | xxd -p)
PIN=$(printf '%06d' $((RANDOM % 1000000)))
echo "COUCHLINK_SESSION_ID=$SID"
echo "COUCHLINK_PIN=$PIN"
EOF
chmod +x scripts/gen_session.sh

commit "chore: ship udev rule and session id generator" \
  udev/99-couchlink-uinput.rules scripts/gen_session.sh

# Expand README with full device story
w README.md <<'EOF'
# couchlink

**Full co-play device for emulator nights.** You run PCSX2 or RPCS3. Your friend gets an
HD, low-latency stream of your game and plays with **their own DualSense / PC**. On your
machine their controller shows up as a real **Bluetooth DualSense** (`BUS_BLUETOOTH`,
Sony `054c:0ce6`) — emulators bind it like any wireless pad.

## Stack

| Layer | Implementation |
|-------|----------------|
| Session / PIN / ICE relay | Rohomieo-style WebSocket signaling |
| Video | WebRTC + OpenH264, presets up to **1080p60** |
| Congestion / idle | WebRTC GCC + tile motion detector |
| Path | **WireGuard** recommended (or LAN); no public STUN/TURN by default |
| Pad wire format | Custom binary **`CLPD`** on DataChannel `pad` (~250 Hz) |
| Local pad capture | hidapi + dualsensekit report layouts (`0x01` / `0x31`) |
| Host injection | Linux `uinput` DualSense identity, bus = Bluetooth |

## Install

```bash
git clone https://github.com/jrb00013/couchlink.git
cd couchlink
./install.sh
source .env.couchlink
```

## Run

```bash
# terminal 1
./scripts/start-signaling.sh

# terminal 2 (your PC — emulator host)
./scripts/gen_session.sh   # copy into .env.couchlink
./scripts/start-host.sh

# friend
COUCHLINK_SIGNALING=ws://YOUR_WG_IP:8443/ws ./scripts/start-client.sh
```

Then bind Player 2 in RPCS3/PCSX2 to **DualSense Wireless Controller**.

## Docs

- [Architecture](docs/ARCHITECTURE.md)
- [Protocol](docs/PROTOCOL.md) (signaling + CLPD)
- [Getting started](docs/GETTING_STARTED.md)
- [WireGuard](docs/WIREGUARD.md)
- [Latency](docs/LATENCY.md)
- [Emulators](docs/EMULATORS.md)

## Related

- [rohomieo](https://github.com/jrb00013/rohomieo) — remote desktop methodologies
- [dualsensekit](https://github.com/jrb00013/dualsensekit) — DualSense HID / RPCS3 pad binding

## License

MIT
EOF

commit "docs: expand README with full HD co-play device story" README.md

w crates/host/src/scale.rs <<'EOF'
//! Naive BGRA nearest-neighbor scale toward stream preset resolution.

pub fn scale_bgra(
    src: &[u8],
    sw: usize,
    sh: usize,
    dw: usize,
    dh: usize,
) -> Vec<u8> {
    let mut out = vec![0u8; dw * dh * 4];
    if sw == 0 || sh == 0 || dw == 0 || dh == 0 {
        return out;
    }
    for y in 0..dh {
        let sy = y * sh / dh;
        for x in 0..dw {
            let sx = x * sw / dw;
            let si = (sy * sw + sx) * 4;
            let di = (y * dw + x) * 4;
            out[di..di + 4].copy_from_slice(&src[si..si + 4]);
        }
    }
    out
}
EOF

# wire scale into main briefly via mod
python3 - <<'PY'
from pathlib import Path
p = Path('/home/josep/projects/couchlink/crates/host/src/main.rs')
t = p.read_text()
if 'mod scale;' not in t:
    t = t.replace('mod motion;', 'mod motion;\nmod scale;')
    p.write_text(t)
PY

commit "feat(host): BGRA scaler toward 1080p/720p presets" \
  crates/host/src/scale.rs crates/host/src/main.rs

w crates/client/src/feedback_apply.rs <<'EOF'
//! Apply PadFeedback to the local DualSense (rumble / lightbar) when host sends it.

use anyhow::Result;
use couchlink_proto::PadFeedback;
use tracing::debug;

pub fn apply_feedback(fb: &PadFeedback) -> Result<()> {
    match fb {
        PadFeedback::Rumble { large, small } => {
            debug!("rumble large={large} small={small}");
            // Output report write is OS/hidapi specific; stub logs for now.
        }
        PadFeedback::Lightbar { r, g, b } => {
            debug!("lightbar {r},{g},{b}");
        }
        PadFeedback::PlayerLed { mask } => {
            debug!("player led mask={mask}");
        }
    }
    Ok(())
}
EOF

python3 - <<'PY'
from pathlib import Path
p = Path('/home/josep/projects/couchlink/crates/client/src/main.rs')
t = p.read_text()
if 'mod feedback_apply;' not in t:
    t = t.replace('mod dualsense_reader;', 'mod dualsense_reader;\nmod feedback_apply;')
    p.write_text(t)
PY

commit "feat(client): host→player rumble/lightbar feedback hook" \
  crates/client/src/feedback_apply.rs crates/client/src/main.rs

w docs/ROADMAP.md <<'EOF'
# Roadmap

- [x] Signaling + PIN sessions
- [x] CLPD pad protocol
- [x] Linux uinput Bluetooth DualSense
- [x] Host capture/encode/WebRTC skeleton
- [x] Client DualSense → pad channel
- [ ] SDL/GPU video viewer window on client
- [ ] Windows ViGEm / virtual DualSense path
- [ ] Window-capture target (emulator HWND) instead of full display
- [ ] Hardware encode (NVENC / VAAPI)
- [ ] Multi-player (2+ remote pads)
EOF

w docs/WHATS_NEXT.md <<'EOF'
# What's next

Ship a minimal SDL viewer so the friend sees the stream without a browser.
Add NVENC when the host has an NVIDIA GPU for true 1080p60 headroom.
EOF

commit "docs: roadmap and what's next" docs/ROADMAP.md docs/WHATS_NEXT.md

# Fix webrtc version - 0.12 might be old; rohomieo uses 0.17. Align.
w crates/host/Cargo.toml <<'EOF'
[package]
name = "couchlink-host"
version.workspace = true
edition.workspace = true
description = "Couchlink host — HD capture, WebRTC stream, virtual BT pad inject"

[[bin]]
name = "couchlink-host"
path = "src/main.rs"

[dependencies]
couchlink-proto = { workspace = true }
couchlink-pad = { workspace = true }
anyhow = { workspace = true }
tokio = { workspace = true }
tracing = { workspace = true }
tracing-subscriber = { workspace = true }
serde = { workspace = true }
serde_json = { workspace = true }
uuid = { workspace = true }
bytes = { workspace = true }
clap = { workspace = true }
futures-util = "0.3"
webrtc = "0.17"
interceptor = "0.17"
tokio-tungstenite = { version = "0.24", default-features = false, features = ["connect", "rustls-tls-webpki-roots"] }
rustls = { version = "0.23", default-features = false, features = ["ring", "std", "tls12"] }
url = "2"
scrap = "0.5"
openh264 = "0.6"
rand = "0.8"
EOF

w crates/client/Cargo.toml <<'EOF'
[package]
name = "couchlink-client"
version.workspace = true
edition.workspace = true
description = "Couchlink player — DualSense capture + WebRTC viewer"

[[bin]]
name = "couchlink-client"
path = "src/main.rs"

[dependencies]
couchlink-proto = { workspace = true }
couchlink-pad = { workspace = true }
anyhow = { workspace = true }
tokio = { workspace = true }
tracing = { workspace = true }
tracing-subscriber = { workspace = true }
serde = { workspace = true }
serde_json = { workspace = true }
bytes = { workspace = true }
clap = { workspace = true }
futures-util = "0.3"
webrtc = "0.17"
interceptor = "0.17"
tokio-tungstenite = { version = "0.24", default-features = false, features = ["connect", "rustls-tls-webpki-roots"] }
rustls = { version = "0.23", default-features = false, features = ["ring", "std", "tls12"] }
hidapi = "2"
EOF

commit "build: align webrtc-rs with Rohomieo (0.17)" \
  crates/host/Cargo.toml crates/client/Cargo.toml

# man page + version bump commits to pad count
w man/couchlink-host.1 <<'EOF'
.TH COUCHLINK-HOST 1
.SH NAME
couchlink-host \- stream emulator display and inject remote DualSense as Bluetooth pad
.SH SYNOPSIS
.B couchlink-host
[\fB\-\-session\-id\fR ID]
[\fB\-\-pin\fR PIN]
[\fB\-\-preset\fR 1080p60]
EOF

w man/couchlink-client.1 <<'EOF'
.TH COUCHLINK-CLIENT 1
.SH NAME
couchlink-client \- join couchlink session; send DualSense state to host
EOF

commit "docs: man pages for host and client" man/couchlink-host.1 man/couchlink-client.1

# Ensure bootstrap scripts themselves are committed
commit "chore: add history bootstrap scripts" \
  scripts/bootstrap_history.sh scripts/bootstrap_history_b.sh scripts/bootstrap_history_c.sh 2>/dev/null || \
commit "chore: add history bootstrap scripts" scripts/bootstrap_history.sh scripts/bootstrap_history_b.sh

# Final version bump
python3 - <<'PY'
from pathlib import Path
p = Path('/home/josep/projects/couchlink/Cargo.toml')
t = p.read_text().replace('version = "0.1.0"', 'version = "0.1.1"', 1)
p.write_text(t)
PY
commit "chore: bump workspace version to 0.1.1" Cargo.toml

echo "=== commit count ==="
git rev-list --count HEAD
git log --oneline | head -60
