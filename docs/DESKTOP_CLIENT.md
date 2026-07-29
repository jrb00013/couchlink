# Desktop player (friend)

Packaged **Couchlink Player** — native video window + DualSense/keyboard. The host still runs on **Linux/WSL** today.

## For friends: download like a normal app

**[FRIEND_INSTALL.md](FRIEND_INSTALL.md)** — one-page “download the `.exe` / `.dmg` / AppImage and go.”

Build installers locally or publish via GitHub Actions (`release-player` workflow → tag `v0.1.1` → **Releases** artifacts).

| Platform | Build on your machine | Friend gets |
|----------|----------------------|-------------|
| **Windows** | `.\packaging\windows\build-installer.ps1` (needs [Inno Setup 6](https://jrsoftware.org/isdl.php)) | **`CouchlinkPlayer-Setup-0.1.1.exe`** — real install wizard, Start Menu, uninstaller |
| **macOS** | `./packaging/macos/build-dmg.sh` | **`CouchlinkPlayer-mac.dmg`** → drag to Applications |
| **Linux** | `./packaging/linux/build-appimage.sh` + `./packaging/linux/build-deb.sh` | **AppImage** or **`.deb`** |

Legacy zip + `install-client.ps1` still works: `.\packaging\windows\build-release.ps1`.

## Join config

On every desktop launch the player **asks for the host join link** (pre-filled from
the last session). Terminal / `./scripts/run.sh client` prompts in the terminal when
credentials are missing — paste the URL, or press Enter and type session/PIN/TURN.

You can still pre-seed config (optional; still shown in the startup prompt):

| OS | File |
|----|------|
| Windows | `%LOCALAPPDATA%\Couchlink\config` |
| Linux | `~/.config/couchlink/config` |
| macOS | `~/Library/Application Support/Couchlink/config` |

```ini
join_url=http://YOUR_HOST:8443/?s=SESSION&p=PIN&auto=1&ws=ws://YOUR_HOST:8443/ws&turn=turn:YOUR_HOST:3478&turnu=...&turnp=...
```

Or run once from a terminal:

```bash
couchlink-client --join-url 'PASTE_FULL_LINK_HERE'
# automation / CI: skip the prompt
couchlink-client --no-prompt --join-url '…'
```

Template: [packaging/config.example](../packaging/config.example)

## Linux desktop entry (manual install)

```bash
cargo build --release -p couchlink-client
install -Dm755 target/release/couchlink-client ~/.local/bin/
install -Dm644 packaging/linux/couchlink-client.desktop ~/.local/share/applications/
```

## Notes

- **Host** is not in these installers (needs Linux `uinput`). Use `./scripts/run.sh host` on the gaming PC.
- Unsigned macOS/Windows builds may show a security prompt; friends use “Open anyway” or you code-sign later.
- AppImage needs `appimagetool` on the machine that *builds* the image, not on the friend’s PC.

See [PLAY_TOGETHER.md](PLAY_TOGETHER.md) for the full cross-world session flow.
