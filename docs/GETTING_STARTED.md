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
