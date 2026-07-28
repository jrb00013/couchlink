# No-computer UX (install app → click → play)

## Who needs what

| Person | What they install | Needs sudo / uinput? |
|--------|-------------------|----------------------|
| **Friend (player)** | `CouchlinkPlayer-Setup.exe` / `.dmg` / AppImage / browser | **Never.** Just paste join link / open URL. |
| **You (host)** | Linux **Couchlink Host** `.deb` (or `install.sh` once) | **Once** at install — unlocks virtual DualSense. Then Apps → **Couchlink Host**. |

`/dev/uinput` is a **kernel** device. No language (C/Rust) can open it without permission. The fix is packaging, not rewriting the pad code in C.

## Native C helper (what we added)

`native/uinput-helper/couchlink-uinput-helper.c` is a tiny C tool used by the host installer:

- `install-rules` — writes the udev rule, reloads udev, adds user to `input` (run via **pkexec** = GUI password, like installing Steam).
- `check` — exits 0 if `/dev/uinput` is already writable (launcher uses this).

It is **not** a permanent setuid backdoor. Prefer: install once → log out → forever normal.

## Friend flow (no computers)

1. Download **Couchlink Player** installer from Releases.
2. Next → Next → paste join link (or save it in config).
3. Open **Couchlink Player** / or just open the join URL in Chrome.

## Host flow (no terminals)

1. Install `couchlink-host_*.deb` (double-click / Software Install — password once).
2. If prompted, open **Couchlink Host Setup** (password once) then **log out and back in**.
3. Apps → **Couchlink Host** — prints join URL / QR → send to friend.
4. Bind Player 2 in the emulator to DualSense Wireless Controller.

Build the host package:

```bash
./packaging/linux/build-host-deb.sh
# → build/couchlink-host_0.1.1_amd64.deb
```

Dev one-shot (what you already did with chmod, but permanent):

```bash
./packaging/linux/install-host-permissions.sh   # GUI pkexec password
# then log out / back in
```

## Why not “pure C uinput forever without install”?

Creating a virtual gamepad is intentionally privileged on Linux/Windows (ViGEm also needs an admin driver install once). Every co-play app (Steam Remote Play, etc.) does **one** elevated install, then normal runs. Same model here.
