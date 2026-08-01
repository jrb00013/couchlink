# Couchlink Player — for friends (download & install)

## From source

```bash
./install.sh              # player + Tailscale
./install.sh --run        # start client — paste the host join URL
./install.sh --online     # same, online mode
```

Host (gaming PC):

```bash
./install.sh --host --online
```

Sign into Tailscale on the **same tailnet** as the host when they send a mesh (`http://100.x…`) link. LAN / Cloudflare links still work either way.

## Packaged download

Get the installer for your OS from the host, or from **GitHub → Releases** on the couchlink repo (tag `v0.1.1` or newer):

| OS | File | What to do |
|----|------|------------|
| **Windows** | `CouchlinkPlayer-Setup-0.1.1.exe` | Double-click → Next → paste join link (optional) → Finish → open **Couchlink Player** |
| **macOS** | `CouchlinkPlayer-mac.dmg` | Open DMG → drag **Couchlink Player** to **Applications** → first open: right-click → **Open** |
| **Linux** | `CouchlinkPlayer-x86_64.AppImage` | `chmod +x` → double-click, **or** `sudo dpkg -i couchlink-player_*_amd64.deb` |

No Rust required for packaged installs. For Tailscale mesh links (`http://100.x…`), install [Tailscale](https://tailscale.com/download) and join the host’s tailnet first.

## Join link

The host sends one long URL. Paste it when the player prompts, or during Windows install on the **Invite link** step. Or put it in the config file:

```ini
join_url=PASTE_THE_FULL_URL_HERE
```

| OS | Config file |
|----|-------------|
| Windows | `%LOCALAPPDATA%\Couchlink\config` |
| macOS | `~/Library/Application Support/Couchlink/config` |
| Linux | `~/.config/couchlink/config` |

Then launch **Couchlink Player**. Plug in a DualSense, or use the keyboard.

## Uninstall (Windows)

Settings → Apps → **Couchlink Player** → Uninstall.

---

Host setup: [PLAY_TOGETHER.md](PLAY_TOGETHER.md) · Mesh: [MESH.md](MESH.md)
