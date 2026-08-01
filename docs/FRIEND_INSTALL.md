# Couchlink Player — for friends (download & install)

## From source

```bash
./install.sh              # player only — no Tailscale required
./install.sh --run        # LAN: paste the host’s LAN join URL
./install.sh --online     # remote: paste whatever URL the host sent
```

Host (gaming PC):

```bash
./install.sh --host --online   # may print http://100.x… if Tailscale is up
```

**Tailscale is only needed** when the join URL is a Tailscale address (`http://100.x…` / `mesh=tailscale`). Same Wi‑Fi and Cloudflare/public links work without it.

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
