# Run DualSense VHID companion (Player 2 for RPCS3/PCSX2)

Requires:
1. [ViGEmBus](https://github.com/nefarius/ViGEmBus/releases) installed once (admin)
2. Rust toolchain with Windows target (`x86_64-pc-windows-msvc`)

```powershell
# From repo root (native Windows or PowerShell in WSL calling Windows cargo):
cargo build -p couchlink-ds-vhid --release
.\target\release\couchlink-ds-vhid.exe

# Optional: rumble feedback to friend (Xbox 360 virtual P2)
.\target\release\couchlink-ds-vhid.exe --backend xbox360
```

Then start `couchlink-host` (WSL or native). WSL Auto connects to `127.0.0.1:39251`.

Emulator binding:
- Player 1 = your physical DualSense on the host
- Player 2 = ViGEm DualShock 4 (default) or Xbox 360 (`--backend xbox360`)
