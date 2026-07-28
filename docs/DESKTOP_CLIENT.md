# Desktop player (friend)

Packaged **Couchlink Player** — native video window + DualSense/keyboard. The host still runs on **Linux/WSL** today.

## Quick install

| Platform | Build (you or CI) | Friend installs |
|----------|-------------------|-----------------|
| **Windows** | `.\packaging\windows\build-release.ps1` | Unzip `build/windows/CouchlinkPlayer-win64.zip`, run **`install-client.ps1`**, paste join URL |
| **Linux** | `./packaging/linux/build-appimage.sh` | Run **`CouchlinkPlayer-*.AppImage`**, set `join_url` in config (below) |
| **macOS** | `./packaging/macos/build-app-bundle.sh` | Drag **`Couchlink Player.app`** to Applications, set config (below) |

Friends do **not** need Rust installed if you give them the zip/AppImage/app bundle.

## Join config (no terminal)

Save the host’s full join link (the same URL as the browser invite) in one line:

| OS | File |
|----|------|
| Windows | `%LOCALAPPDATA%\Couchlink\config` |
| Linux | `~/.config/couchlink/config` |
| macOS | `~/Library/Application Support/Couchlink/config` |

Contents:

```ini
join_url=http://YOUR_HOST:8443/?s=SESSION&p=PIN&auto=1&ws=ws://YOUR_HOST:8443/ws&turn=turn:YOUR_HOST:3478&turnu=...&turnp=...
```

Or run once from a terminal:

```bash
couchlink-client --join-url 'PASTE_FULL_LINK_HERE'
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
