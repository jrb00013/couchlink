# Couchlink Player — for friends (download & install)

You only need this if the host sent you a **join link** and you want the **native app** instead of a browser.

## Download

Get the installer for your OS from the host, or from **GitHub → Releases** on the couchlink repo (tag `v0.1.1` or newer):

| OS | File | What to do |
|----|------|------------|
| **Windows** | `CouchlinkPlayer-Setup-0.1.1.exe` | Double-click → Next → paste join link (optional) → Finish → open **Couchlink Player** from Start Menu |
| **macOS** | `CouchlinkPlayer-mac.dmg` | Open DMG → drag **Couchlink Player** to **Applications** → first open: right-click → **Open** (unsigned app) |
| **Linux** | `CouchlinkPlayer-x86_64.AppImage` | `chmod +x` → double-click, **or** install `couchlink-player_*_amd64.deb` with `sudo dpkg -i …` |

No Rust, no terminal, no git.

## Join link

The host sends one long URL (Discord, text, etc.). During Windows install you can paste it on the **Invite link** step. Otherwise put it in the config file:

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

Host setup: [PLAY_TOGETHER.md](PLAY_TOGETHER.md)
