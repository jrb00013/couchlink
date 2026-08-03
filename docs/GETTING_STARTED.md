# Getting started

## You (host) — machine running PCSX2 / RPCS3

```bash
git clone https://github.com/jrb00013/couchlink.git
cd couchlink
./install.sh
source .env.couchlink

couchlink-signaling &
./scripts/start-turn.sh &   # local TURN relay — makes internet play automatic, no VPN
couchlink-host \
  --session-id friends-night \
  --pin 482193 \
  --preset 1080p60 \
  --emulator rpcs3
```

Ensure your user can write `/dev/uinput` (install udev rule from `install.sh`).
On native Windows, install [ViGEmBus](https://github.com/nefarius/ViGEmBus/releases)
once; optional DualSense VHID companion for real `054c:0ce6` identity.

## Friend (player) — browser (recommended)

1. Open the **join URL / QR** printed by `couchlink-host`, or go to `http://HOST:8443`.
2. Press any button on your DualSense so the browser unlocks Gamepad API.
3. Click **Join session** (auto if the invite link has `?s=&p=&auto=1`).
4. You should see the HD stream; pad state streams as `CLPD` onto the host virtual pad
   (Linux DualSense / Windows VHID or ViGEm).

## Friend — native client (optional)

See **[Desktop player install](docs/DESKTOP_CLIENT.md)** (Windows / Linux AppImage / macOS app) and **[Playing together across the internet](docs/PLAY_TOGETHER.md)**.

```bash
./install.sh
couchlink-client --join-url 'PASTE_HOST_JOIN_LINK'
# or set join_url= in the platform config file (see DESKTOP_CLIENT.md)
```

## Bind in the emulator

- **RPCS3**: Player 2 → DualSense Wireless Controller (the virtual BT one)
- **PCSX2**: Controllers → DualSense / SDL → select the couchlink virtual pad

Helpers: `adapters/rpcs3/` and `adapters/pcsx2/`.
