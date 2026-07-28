#!/usr/bin/env bash
# Creates couchlink source tree and ~50 thematic commits.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

commit() {
  local msg="$1"
  shift
  git add "$@"
  # Skip empty
  if git diff --cached --quiet; then
    return 0
  fi
  git commit -m "$msg"
}

# ---------- helpers to write files ----------
w() {
  local path="$1"
  mkdir -p "$(dirname "$path")"
  cat > "$path"
}

############################################
# 01 — license + ignore
############################################
w LICENSE <<'EOF'
MIT License

Copyright (c) 2026 jrb00013

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.
EOF

w .gitignore <<'EOF'
/target/
**/node_modules/
web/dist/
infra/certs/*.pem
infra/wireguard/keys/
var/
.local/
.env.couchlink
.env
.DS_Store
.idea/
.vscode/
*.key
*.pub
python/.venv/
__pycache__/
*.pyc
EOF

commit "chore: add MIT license and gitignore" LICENSE .gitignore

############################################
# 02 — README skeleton
############################################
w README.md <<'EOF'
# couchlink

**HD, low-latency co-play for emulators.** You host PCSX2 / RPCS3 (or any game window).
Your friend streams your game screen over WebRTC and plays with their own DualSense —
on your machine it shows up as a **Bluetooth DualSense** (`BUS_BLUETOOTH`, Sony VID/PID),
so emulators bind it like a real pad.

Built with the same session / signaling / WebRTC methodologies as [Rohomieo](https://github.com/jrb00013/rohomieo),
and DualSense HID report layouts from [dualsensekit](https://github.com/jrb00013/dualsensekit).

## Why

| Piece | How |
|-------|-----|
| Video | WebRTC + H.264, adaptive FPS (Rohomieo-style GCC + motion idle) |
| Transport | Peer-to-peer media; signaling only for SDP/ICE |
| Path | WireGuard LAN recommended (no public STUN/TURN required) |
| Pad | Custom binary `CLPD` frames on DataChannel `pad` (~250 Hz) |
| Host injection | Linux `uinput` device: name/VID/PID of DualSense, bus = Bluetooth |
| Local capture | hidapi / dualsensekit-compatible USB (`0x01`) or BT (`0x31`) reports |

## Quick start

```bash
./install.sh
source .env.couchlink
couchlink-signaling &
couchlink-host --session-id demo --pin 123456 --preset 1080p60
# friend:
couchlink-client --signaling wss://YOU:8443 --session-id demo --pin 123456
```

See [docs/GETTING_STARTED.md](docs/GETTING_STARTED.md).

## License

MIT
EOF

commit "docs: add project README overview" README.md

############################################
# 03 — workspace
############################################
w Cargo.toml <<'EOF'
[workspace]
resolver = "2"
members = [
  "crates/proto",
  "crates/pad",
  "crates/signaling",
  "crates/host",
  "crates/client",
]
default-members = ["crates/signaling", "crates/host", "crates/client"]

[workspace.package]
version = "0.1.0"
edition = "2021"
license = "MIT"
repository = "https://github.com/jrb00013/couchlink"
authors = ["jrb00013"]

[workspace.dependencies]
serde = { version = "1", features = ["derive"] }
serde_json = "1"
tokio = { version = "1", features = ["full"] }
tracing = "0.1"
tracing-subscriber = { version = "0.3", features = ["env-filter"] }
anyhow = "1"
thiserror = "2"
uuid = { version = "1", features = ["v4", "serde"] }
chrono = { version = "0.4", features = ["serde"] }
bytes = "1"
clap = { version = "4", features = ["derive", "env"] }
couchlink-proto = { path = "crates/proto" }
couchlink-pad = { path = "crates/pad" }
EOF

commit "build: scaffold Rust workspace" Cargo.toml

############################################
# 04 — proto crate skeleton
############################################
w crates/proto/Cargo.toml <<'EOF'
[package]
name = "couchlink-proto"
version.workspace = true
edition.workspace = true
description = "Couchlink wire protocol — signaling JSON + binary pad frames"

[dependencies]
serde = { workspace = true }
serde_json = { workspace = true }
bytes = { workspace = true }
thiserror = { workspace = true }
EOF

w crates/proto/src/lib.rs <<'EOF'
//! Couchlink wire protocol shared by signaling, host, and client.
//!
//! Methodologies mirror Rohomieo: tagged JSON `type` discriminators in snake_case
//! for WebSocket signaling; media stays peer-to-peer. Pad state uses a compact
//! binary frame (`CLPD`) on the WebRTC DataChannel named `pad`.

pub mod pad_frame;
pub mod signal;

pub use pad_frame::{PadFeedback, PadFrame, PAD_CHANNEL, PAD_MAGIC};
pub use signal::{Role, SignalMessage, StreamPreset};
EOF

commit "feat(proto): add protocol crate skeleton" crates/proto/Cargo.toml crates/proto/src/lib.rs

############################################
# 05 — signaling messages (Rohomieo pattern)
############################################
w crates/proto/src/signal.rs <<'EOF'
//! WebSocket signaling envelope — Rohomieo methodology adapted for co-play.
//! Host registers with session_id + PIN; friend registers as player.
//! Offer/answer/ICE relay only; video+pad never transit the signaling server.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SignalMessage {
    RegisterHost {
        session_id: String,
        pin: String,
        device_name: Option<String>,
        /// e.g. "1080p60", "720p60", "720p30"
        preset: Option<String>,
        emulator: Option<String>,
    },
    RegisterPlayer {
        session_id: String,
        pin: String,
        player_name: Option<String>,
    },
    Registered {
        role: Role,
        session_id: String,
    },
    Error {
        message: String,
    },
    Offer {
        sdp: String,
    },
    Answer {
        sdp: String,
    },
    IceCandidate {
        candidate: String,
        #[serde(rename = "sdpMid")]
        sdp_mid: Option<String>,
        #[serde(rename = "sdpMLineIndex")]
        sdp_mline_index: Option<u16>,
    },
    Heartbeat,
    Pong,
    PeerJoined {
        role: Role,
    },
    PeerLeft,
    /// Host announces stream ready (codec / resolution).
    StreamInfo {
        width: u32,
        height: u32,
        fps: u32,
        codec: String,
    },
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    Host,
    Player,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StreamPreset {
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    pub bitrate_kbps: u32,
}

impl StreamPreset {
    pub const P1080_60: Self = Self {
        width: 1920,
        height: 1080,
        fps: 60,
        bitrate_kbps: 12_000,
    };
    pub const P1080_30: Self = Self {
        width: 1920,
        height: 1080,
        fps: 30,
        bitrate_kbps: 8_000,
    };
    pub const P720_60: Self = Self {
        width: 1280,
        height: 720,
        fps: 60,
        bitrate_kbps: 8_000,
    };
    pub const P720_30: Self = Self {
        width: 1280,
        height: 720,
        fps: 30,
        bitrate_kbps: 4_000,
    };

    pub fn parse(name: &str) -> Option<Self> {
        match name.trim().to_ascii_lowercase().as_str() {
            "1080p60" | "hd60" => Some(Self::P1080_60),
            "1080p30" | "hd30" => Some(Self::P1080_30),
            "720p60" => Some(Self::P720_60),
            "720p30" | "default" => Some(Self::P720_30),
            _ => None,
        }
    }
}

impl SignalMessage {
    pub fn to_json(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string(self)
    }

    pub fn from_json(s: &str) -> Result<Self, serde_json::Error> {
        serde_json::from_str(s)
    }
}
EOF

commit "feat(proto): Rohomieo-style signaling messages + HD presets" crates/proto/src/signal.rs

############################################
# 06 — binary pad frame (custom proto)
############################################
w crates/proto/src/pad_frame.rs <<'EOF'
//! Binary pad protocol (`CLPD`) — lower latency than JSON for ~250 Hz DualSense state.
//! Layout inspired by dualsensekit USB/BT input reports, normalized for the wire.

use bytes::{Buf, BufMut, BytesMut};
use thiserror::Error;

/// WebRTC DataChannel label for pad traffic.
pub const PAD_CHANNEL: &str = "pad";
/// ASCII magic.
pub const PAD_MAGIC: &[u8; 4] = b"CLPD";
pub const PAD_VERSION: u8 = 1;
/// Packed frame size (header + body).
pub const PAD_FRAME_LEN: usize = 4 + 1 + 4 + 4 + 6 + 2 + 8;

#[derive(Debug, Error)]
pub enum PadCodecError {
    #[error("buffer too short")]
    Short,
    #[error("bad magic")]
    BadMagic,
    #[error("unsupported version {0}")]
    BadVersion(u8),
}

/// Normalized DualSense-like state (host injects this into virtual BT pad).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct PadFrame {
    pub seq: u32,
    pub buttons: u32,
    pub lx: u8,
    pub ly: u8,
    pub rx: u8,
    pub ry: u8,
    pub l2: u8,
    pub r2: u8,
    /// Gyroscope (optional; zero if unused)
    pub gx: i16,
    pub gy: i16,
    pub gz: i16,
    pub touch_active: u8,
    pub touch_x: u16,
    pub touch_y: u16,
}

/// Host → player haptic / lightbar feedback (JSON on same channel, type tag).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum PadFeedback {
    Rumble { large: u8, small: u8 },
    Lightbar { r: u8, g: u8, b: u8 },
    PlayerLed { mask: u8 },
}

// Button bits — match common DS layouts (dualsensekit / hid-playstation style).
pub mod buttons {
    pub const SQUARE: u32 = 1 << 0;
    pub const CROSS: u32 = 1 << 1;
    pub const CIRCLE: u32 = 1 << 2;
    pub const TRIANGLE: u32 = 1 << 3;
    pub const L1: u32 = 1 << 4;
    pub const R1: u32 = 1 << 5;
    pub const L2: u32 = 1 << 6;
    pub const R2: u32 = 1 << 7;
    pub const CREATE: u32 = 1 << 8;
    pub const OPTIONS: u32 = 1 << 9;
    pub const L3: u32 = 1 << 10;
    pub const R3: u32 = 1 << 11;
    pub const PS: u32 = 1 << 12;
    pub const TOUCH: u32 = 1 << 13;
    pub const MUTE: u32 = 1 << 14;
    pub const DPAD_UP: u32 = 1 << 16;
    pub const DPAD_DOWN: u32 = 1 << 17;
    pub const DPAD_LEFT: u32 = 1 << 18;
    pub const DPAD_RIGHT: u32 = 1 << 19;
}

impl PadFrame {
    pub fn encode(&self, out: &mut BytesMut) {
        out.reserve(PAD_FRAME_LEN);
        out.put_slice(PAD_MAGIC);
        out.put_u8(PAD_VERSION);
        out.put_u32_le(self.seq);
        out.put_u32_le(self.buttons);
        out.put_u8(self.lx);
        out.put_u8(self.ly);
        out.put_u8(self.rx);
        out.put_u8(self.ry);
        out.put_u8(self.l2);
        out.put_u8(self.r2);
        out.put_i16_le(self.gx);
        out.put_i16_le(self.gy);
        out.put_i16_le(self.gz);
        out.put_u8(self.touch_active);
        out.put_u16_le(self.touch_x);
        out.put_u16_le(self.touch_y);
        // pad to fixed size with reserved
        out.put_u8(0);
    }

    pub fn decode(mut buf: &[u8]) -> Result<Self, PadCodecError> {
        if buf.len() < PAD_FRAME_LEN {
            return Err(PadCodecError::Short);
        }
        let mut magic = [0u8; 4];
        buf.copy_to_slice(&mut magic);
        if &magic != PAD_MAGIC {
            return Err(PadCodecError::BadMagic);
        }
        let ver = buf.get_u8();
        if ver != PAD_VERSION {
            return Err(PadCodecError::BadVersion(ver));
        }
        Ok(Self {
            seq: buf.get_u32_le(),
            buttons: buf.get_u32_le(),
            lx: buf.get_u8(),
            ly: buf.get_u8(),
            rx: buf.get_u8(),
            ry: buf.get_u8(),
            l2: buf.get_u8(),
            r2: buf.get_u8(),
            gx: buf.get_i16_le(),
            gy: buf.get_i16_le(),
            gz: buf.get_i16_le(),
            touch_active: buf.get_u8(),
            touch_x: buf.get_u16_le(),
            touch_y: buf.get_u16_le(),
        })
    }

    pub fn neutral() -> Self {
        Self {
            lx: 128,
            ly: 128,
            rx: 128,
            ry: 128,
            ..Default::default()
        }
    }
}
EOF

# update lib.rs already done
commit "feat(proto): custom CLPD binary pad frame codec" crates/proto/src/pad_frame.rs

############################################
# 07 — proto tests
############################################
w crates/proto/src/lib.rs <<'EOF'
//! Couchlink wire protocol shared by signaling, host, and client.
//!
//! Methodologies mirror Rohomieo: tagged JSON `type` discriminators in snake_case
//! for WebSocket signaling; media stays peer-to-peer. Pad state uses a compact
//! binary frame (`CLPD`) on the WebRTC DataChannel named `pad`.

pub mod pad_frame;
pub mod signal;

pub use pad_frame::{PadFeedback, PadFrame, PAD_CHANNEL, PAD_MAGIC};
pub use signal::{Role, SignalMessage, StreamPreset};

#[cfg(test)]
mod tests {
    use super::*;
    use bytes::BytesMut;

    #[test]
    fn signal_register_host_roundtrip() {
        let msg = SignalMessage::RegisterHost {
            session_id: "abc".into(),
            pin: "123456".into(),
            device_name: Some("desk".into()),
            preset: Some("1080p60".into()),
            emulator: Some("rpcs3".into()),
        };
        let json = msg.to_json().unwrap();
        let back = SignalMessage::from_json(&json).unwrap();
        assert!(matches!(back, SignalMessage::RegisterHost { .. }));
    }

    #[test]
    fn pad_frame_roundtrip() {
        let mut f = PadFrame::neutral();
        f.seq = 42;
        f.buttons = pad_frame::buttons::CROSS | pad_frame::buttons::R1;
        f.l2 = 200;
        let mut buf = BytesMut::new();
        f.encode(&mut buf);
        let back = PadFrame::decode(&buf).unwrap();
        assert_eq!(back, f);
    }

    #[test]
    fn preset_parse() {
        assert_eq!(StreamPreset::parse("1080p60").unwrap().fps, 60);
        assert_eq!(StreamPreset::parse("720p30").unwrap().width, 1280);
    }
}
EOF

commit "test(proto): roundtrip signaling and CLPD frames" crates/proto/src/lib.rs

############################################
# 08 — pad crate: DualSense constants (dualsensekit)
############################################
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

[target.'cfg(target_os = "linux")'.dependencies]
nix = { version = "0.29", features = ["ioctl", "fs"] }
EOF

w crates/pad/src/lib.rs <<'EOF'
//! Pad stack: parse real DualSense HID reports (dualsensekit layouts) and
//! inject a virtual DualSense that announces itself as Bluetooth.

pub mod dualsense;
pub mod parse;
pub mod virtual_pad;

pub use dualsense::{SONY_VID, PID_DUALSENSE, PID_DUALSENSE_EDGE};
pub use parse::parse_input_report;
pub use virtual_pad::{VirtualPad, VirtualPadConfig};
EOF

w crates/pad/src/dualsense.rs <<'EOF'
//! Constants and report IDs — aligned with dualsensekit PROTOCOL.md.

pub const SONY_VID: u16 = 0x054C;
pub const PID_DUALSENSE: u16 = 0x0CE6;
pub const PID_DUALSENSE_EDGE: u16 = 0x0DF2;

pub const FEATURE_BT_CONTROL: u8 = 0x08;
pub const FEATURE_PAIRING: u8 = 0x09;
pub const FEATURE_BT_PAIRING: u8 = 0x0A;
pub const FEATURE_FIRMWARE: u8 = 0x20;

pub const INPUT_USB: u8 = 0x01;
pub const INPUT_BT: u8 = 0x31;
pub const OUTPUT_USB: u8 = 0x02;

pub const USB_REPORT_LEN: usize = 64;
pub const BT_REPORT_LEN: usize = 78;

/// Product string emulators expect.
pub const PRODUCT_NAME: &str = "DualSense Wireless Controller";
EOF

commit "feat(pad): DualSense VID/PID and report IDs from dualsensekit" crates/pad/Cargo.toml crates/pad/src/lib.rs crates/pad/src/dualsense.rs

############################################
# 09 — parse USB/BT reports → PadFrame
############################################
w crates/pad/src/parse.rs <<'EOF'
//! Parse DualSense USB (0x01) / Bluetooth (0x31) input reports into PadFrame.
//! Button nibble / stick offsets follow hid-playstation / dualsensekit community layouts.

use couchlink_proto::pad_frame::buttons;
use couchlink_proto::PadFrame;

use crate::dualsense::{INPUT_BT, INPUT_USB};

pub fn parse_input_report(raw: &[u8]) -> Option<PadFrame> {
    if raw.is_empty() {
        return None;
    }
    let (body, is_bt) = match raw[0] {
        INPUT_USB => (&raw[1..], false),
        INPUT_BT => {
            // BT report: id, then often a 0x01 tag, then USB-like payload
            if raw.len() < 3 {
                return None;
            }
            let start = if raw.get(1) == Some(&0x01) { 2 } else { 1 };
            (&raw[start..], true)
        }
        _ => {
            // bare USB payload without report id
            if raw.len() >= 9 {
                (raw, false)
            } else {
                return None;
            }
        }
    };
    if body.len() < 10 {
        return None;
    }

    let lx = body[0];
    let ly = body[1];
    let rx = body[2];
    let ry = body[3];
    let l2 = body[4];
    let r2 = body[5];
    let buttons_l = body[7];
    let buttons_h = body[8];
    let buttons_extra = body.get(9).copied().unwrap_or(0);

    let dpad = buttons_l & 0x0F;
    let mut buttons = 0u32;
    // face buttons in high nibble of buttons_l
    if buttons_l & 0x10 != 0 {
        buttons |= buttons::SQUARE;
    }
    if buttons_l & 0x20 != 0 {
        buttons |= buttons::CROSS;
    }
    if buttons_l & 0x40 != 0 {
        buttons |= buttons::CIRCLE;
    }
    if buttons_l & 0x80 != 0 {
        buttons |= buttons::TRIANGLE;
    }
    if buttons_h & 0x01 != 0 {
        buttons |= buttons::L1;
    }
    if buttons_h & 0x02 != 0 {
        buttons |= buttons::R1;
    }
    if buttons_h & 0x04 != 0 {
        buttons |= buttons::L2;
    }
    if buttons_h & 0x08 != 0 {
        buttons |= buttons::R2;
    }
    if buttons_h & 0x10 != 0 {
        buttons |= buttons::CREATE;
    }
    if buttons_h & 0x20 != 0 {
        buttons |= buttons::OPTIONS;
    }
    if buttons_h & 0x40 != 0 {
        buttons |= buttons::L3;
    }
    if buttons_h & 0x80 != 0 {
        buttons |= buttons::R3;
    }
    if buttons_extra & 0x01 != 0 {
        buttons |= buttons::PS;
    }
    if buttons_extra & 0x02 != 0 {
        buttons |= buttons::TOUCH;
    }
    if buttons_extra & 0x04 != 0 {
        buttons |= buttons::MUTE;
    }

    match dpad {
        0 => buttons |= buttons::DPAD_UP,
        1 => buttons |= buttons::DPAD_UP | buttons::DPAD_RIGHT,
        2 => buttons |= buttons::DPAD_RIGHT,
        3 => buttons |= buttons::DPAD_DOWN | buttons::DPAD_RIGHT,
        4 => buttons |= buttons::DPAD_DOWN,
        5 => buttons |= buttons::DPAD_DOWN | buttons::DPAD_LEFT,
        6 => buttons |= buttons::DPAD_LEFT,
        7 => buttons |= buttons::DPAD_UP | buttons::DPAD_LEFT,
        _ => {}
    }

    let _ = is_bt;
    Some(PadFrame {
        seq: 0,
        buttons,
        lx,
        ly,
        rx,
        ry,
        l2,
        r2,
        gx: 0,
        gy: 0,
        gz: 0,
        touch_active: 0,
        touch_x: 0,
        touch_y: 0,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_neutral_usb() {
        let mut raw = vec![0u8; 64];
        raw[0] = INPUT_USB;
        raw[1] = 128;
        raw[2] = 128;
        raw[3] = 128;
        raw[4] = 128;
        let f = parse_input_report(&raw).unwrap();
        assert_eq!(f.lx, 128);
        assert_eq!(f.ly, 128);
    }
}
EOF

commit "feat(pad): parse DualSense USB and Bluetooth input reports" crates/pad/src/parse.rs

############################################
# 10 — virtual Bluetooth DualSense via uinput
############################################
w crates/pad/src/virtual_pad.rs <<'EOF'
//! Virtual DualSense presented as a **Bluetooth** gamepad on the host.
//!
//! Linux: `uinput` with `BUS_BLUETOOTH`, Sony VID/PID, and DualSense product name
//! so PCSX2 / RPCS3 enumerate it like a real wireless pad (same idea dualsensekit
//! uses when binding RPCS3 player slots to DualSense HID endpoints).

use anyhow::{bail, Context, Result};
use couchlink_proto::pad_frame::buttons;
use couchlink_proto::PadFrame;
use tracing::info;

use crate::dualsense::{PID_DUALSENSE, PRODUCT_NAME, SONY_VID};

#[derive(Debug, Clone)]
pub struct VirtualPadConfig {
    pub name: String,
    pub vendor: u16,
    pub product: u16,
    pub version: u16,
    /// When true, set bus type to Bluetooth so udev/emulators treat it as wireless.
    pub as_bluetooth: bool,
}

impl Default for VirtualPadConfig {
    fn default() -> Self {
        Self {
            name: PRODUCT_NAME.into(),
            vendor: SONY_VID,
            product: PID_DUALSENSE,
            version: 0x0111,
            as_bluetooth: true,
        }
    }
}

pub struct VirtualPad {
    #[cfg(target_os = "linux")]
    inner: linux::LinuxUInput,
    #[cfg(not(target_os = "linux"))]
    _cfg: VirtualPadConfig,
}

impl VirtualPad {
    pub fn create(cfg: VirtualPadConfig) -> Result<Self> {
        #[cfg(target_os = "linux")]
        {
            let inner = linux::LinuxUInput::create(&cfg)?;
            info!(
                "virtual pad ready: '{}' vid={:04x} pid={:04x} bus={}",
                cfg.name,
                cfg.vendor,
                cfg.product,
                if cfg.as_bluetooth {
                    "bluetooth"
                } else {
                    "usb"
                }
            );
            Ok(Self { inner })
        }
        #[cfg(not(target_os = "linux"))]
        {
            let _ = cfg;
            bail!("virtual Bluetooth pad injection is currently implemented for Linux uinput; Windows ViGEm path planned")
        }
    }

    pub fn apply(&mut self, frame: &PadFrame) -> Result<()> {
        #[cfg(target_os = "linux")]
        {
            self.inner.apply(frame)
        }
        #[cfg(not(target_os = "linux"))]
        {
            let _ = frame;
            Ok(())
        }
    }
}

#[cfg(target_os = "linux")]
mod linux {
    use super::*;
    use std::fs::OpenOptions;
    use std::io::Write;
    use std::os::fd::AsRawFd;
    use std::os::unix::fs::OpenOptionsExt;

    // linux/input-event-codes.h / uinput.h subset
    const BUS_USB: u16 = 0x03;
    const BUS_BLUETOOTH: u16 = 0x05;
    const EV_SYN: u16 = 0x00;
    const EV_KEY: u16 = 0x01;
    const EV_ABS: u16 = 0x03;
    const SYN_REPORT: u16 = 0;
    const UI_SET_EVBIT: u64 = 0x40045564;
    const UI_SET_KEYBIT: u64 = 0x40045565;
    const UI_SET_ABSBIT: u64 = 0x40045567;
    const UI_DEV_SETUP: u64 = 0x405c5503;
    const UI_DEV_CREATE: u64 = 0x00005501;
    const UI_DEV_DESTROY: u64 = 0x00005502;

    // Buttons
    const BTN_SOUTH: u16 = 0x130;
    const BTN_EAST: u16 = 0x131;
    const BTN_NORTH: u16 = 0x133;
    const BTN_WEST: u16 = 0x134;
    const BTN_TL: u16 = 0x136;
    const BTN_TR: u16 = 0x137;
    const BTN_TL2: u16 = 0x138;
    const BTN_TR2: u16 = 0x139;
    const BTN_SELECT: u16 = 0x13a;
    const BTN_START: u16 = 0x13b;
    const BTN_MODE: u16 = 0x13c;
    const BTN_THUMBL: u16 = 0x13d;
    const BTN_THUMBR: u16 = 0x13e;
    const BTN_DPAD_UP: u16 = 0x220;
    const BTN_DPAD_DOWN: u16 = 0x221;
    const BTN_DPAD_LEFT: u16 = 0x222;
    const BTN_DPAD_RIGHT: u16 = 0x223;

    const ABS_X: u16 = 0x00;
    const ABS_Y: u16 = 0x01;
    const ABS_RX: u16 = 0x03;
    const ABS_RY: u16 = 0x04;
    const ABS_Z: u16 = 0x02; // L2
    const ABS_RZ: u16 = 0x05; // R2

    #[repr(C)]
    struct InputId {
        bustype: u16,
        vendor: u16,
        product: u16,
        version: u16,
    }

    #[repr(C)]
    struct UinputSetup {
        id: InputId,
        name: [u8; 80],
        ff_effects_max: u32,
    }

    #[repr(C)]
    struct InputEvent {
        time_sec: usize,
        time_usec: usize,
        type_: u16,
        code: u16,
        value: i32,
    }

    pub struct LinuxUInput {
        file: std::fs::File,
    }

    impl LinuxUInput {
        pub fn create(cfg: &VirtualPadConfig) -> Result<Self> {
            let file = OpenOptions::new()
                .read(true)
                .write(true)
                .custom_flags(libc_nonblock())
                .open("/dev/uinput")
                .context("open /dev/uinput (need uinput module + permissions)")?;

            unsafe {
                ioctl_set(file.as_raw_fd(), UI_SET_EVBIT, EV_KEY as u64)?;
                ioctl_set(file.as_raw_fd(), UI_SET_EVBIT, EV_ABS as u64)?;
                ioctl_set(file.as_raw_fd(), UI_SET_EVBIT, EV_SYN as u64)?;
                for code in [
                    BTN_SOUTH, BTN_EAST, BTN_NORTH, BTN_WEST, BTN_TL, BTN_TR, BTN_TL2, BTN_TR2,
                    BTN_SELECT, BTN_START, BTN_MODE, BTN_THUMBL, BTN_THUMBR, BTN_DPAD_UP,
                    BTN_DPAD_DOWN, BTN_DPAD_LEFT, BTN_DPAD_RIGHT,
                ] {
                    ioctl_set(file.as_raw_fd(), UI_SET_KEYBIT, code as u64)?;
                }
                for code in [ABS_X, ABS_Y, ABS_RX, ABS_RY, ABS_Z, ABS_RZ] {
                    ioctl_set(file.as_raw_fd(), UI_SET_ABSBIT, code as u64)?;
                }
            }

            let mut setup: UinputSetup = unsafe { std::mem::zeroed() };
            setup.id.bustype = if cfg.as_bluetooth {
                BUS_BLUETOOTH
            } else {
                BUS_USB
            };
            setup.id.vendor = cfg.vendor;
            setup.id.product = cfg.product;
            setup.id.version = cfg.version;
            let name_bytes = cfg.name.as_bytes();
            let n = name_bytes.len().min(79);
            setup.name[..n].copy_from_slice(&name_bytes[..n]);

            unsafe {
                let ret = libc_ioctl(
                    file.as_raw_fd(),
                    UI_DEV_SETUP,
                    &setup as *const _ as u64,
                );
                if ret < 0 {
                    bail!("UI_DEV_SETUP failed");
                }
                let ret = libc_ioctl(file.as_raw_fd(), UI_DEV_CREATE, 0);
                if ret < 0 {
                    bail!("UI_DEV_CREATE failed");
                }
            }

            // Give udev a moment to create /dev/input/event*
            std::thread::sleep(std::time::Duration::from_millis(100));
            Ok(Self { file })
        }

        pub fn apply(&mut self, frame: &PadFrame) -> Result<()> {
            let b = frame.buttons;
            self.emit_key(BTN_WEST, b & buttons::SQUARE != 0)?;
            self.emit_key(BTN_SOUTH, b & buttons::CROSS != 0)?;
            self.emit_key(BTN_EAST, b & buttons::CIRCLE != 0)?;
            self.emit_key(BTN_NORTH, b & buttons::TRIANGLE != 0)?;
            self.emit_key(BTN_TL, b & buttons::L1 != 0)?;
            self.emit_key(BTN_TR, b & buttons::R1 != 0)?;
            self.emit_key(BTN_TL2, b & buttons::L2 != 0)?;
            self.emit_key(BTN_TR2, b & buttons::R2 != 0)?;
            self.emit_key(BTN_SELECT, b & buttons::CREATE != 0)?;
            self.emit_key(BTN_START, b & buttons::OPTIONS != 0)?;
            self.emit_key(BTN_THUMBL, b & buttons::L3 != 0)?;
            self.emit_key(BTN_THUMBR, b & buttons::R3 != 0)?;
            self.emit_key(BTN_MODE, b & buttons::PS != 0)?;
            self.emit_key(BTN_DPAD_UP, b & buttons::DPAD_UP != 0)?;
            self.emit_key(BTN_DPAD_DOWN, b & buttons::DPAD_DOWN != 0)?;
            self.emit_key(BTN_DPAD_LEFT, b & buttons::DPAD_LEFT != 0)?;
            self.emit_key(BTN_DPAD_RIGHT, b & buttons::DPAD_RIGHT != 0)?;

            self.emit_abs(ABS_X, frame.lx as i32)?;
            self.emit_abs(ABS_Y, frame.ly as i32)?;
            self.emit_abs(ABS_RX, frame.rx as i32)?;
            self.emit_abs(ABS_RY, frame.ry as i32)?;
            self.emit_abs(ABS_Z, frame.l2 as i32)?;
            self.emit_abs(ABS_RZ, frame.r2 as i32)?;
            self.emit(EV_SYN, SYN_REPORT, 0)?;
            Ok(())
        }

        fn emit_key(&mut self, code: u16, down: bool) -> Result<()> {
            self.emit(EV_KEY, code, if down { 1 } else { 0 })
        }

        fn emit_abs(&mut self, code: u16, value: i32) -> Result<()> {
            self.emit(EV_ABS, code, value)
        }

        fn emit(&mut self, type_: u16, code: u16, value: i32) -> Result<()> {
            let ev = InputEvent {
                time_sec: 0,
                time_usec: 0,
                type_,
                code,
                value,
            };
            let bytes = unsafe {
                std::slice::from_raw_parts(
                    &ev as *const _ as *const u8,
                    std::mem::size_of::<InputEvent>(),
                )
            };
            self.file.write_all(bytes)?;
            Ok(())
        }
    }

    impl Drop for LinuxUInput {
        fn drop(&mut self) {
            unsafe {
                let _ = libc_ioctl(self.file.as_raw_fd(), UI_DEV_DESTROY, 0);
            }
        }
    }

    fn libc_nonblock() -> i32 {
        0 // blocking is fine for injection
    }

    unsafe fn ioctl_set(fd: i32, req: u64, val: u64) -> Result<()> {
        let ret = libc_ioctl(fd, req, val);
        if ret < 0 {
            bail!("ioctl 0x{req:x} failed");
        }
        Ok(())
    }

    // Minimal ioctl without pulling full libc crate conflict — use nix
    unsafe fn libc_ioctl(fd: i32, req: u64, arg: u64) -> i32 {
        nix::libc::ioctl(fd, req as _, arg)
    }
}
EOF

commit "feat(pad): virtual DualSense on BUS_BLUETOOTH via uinput" crates/pad/src/virtual_pad.rs

############################################
# 11 — pad abs setup ranges (follow-up polish)
############################################
w crates/pad/src/absinfo.rs <<'EOF'
//! Absolute axis ranges for sticks (0–255) and triggers (0–255).
//! Applied when creating the uinput device so emulators see DualSense-like ranges.

#[derive(Clone, Copy)]
pub struct AbsRange {
    pub min: i32,
    pub max: i32,
    pub fuzz: i32,
    pub flat: i32,
}

pub const STICK: AbsRange = AbsRange {
    min: 0,
    max: 255,
    fuzz: 0,
    flat: 15,
};

pub const TRIGGER: AbsRange = AbsRange {
    min: 0,
    max: 255,
    fuzz: 0,
    flat: 0,
};
EOF

w crates/pad/src/lib.rs <<'EOF'
//! Pad stack: parse real DualSense HID reports (dualsensekit layouts) and
//! inject a virtual DualSense that announces itself as Bluetooth.

pub mod absinfo;
pub mod dualsense;
pub mod parse;
pub mod virtual_pad;

pub use dualsense::{PID_DUALSENSE, PID_DUALSENSE_EDGE, SONY_VID};
pub use parse::parse_input_report;
pub use virtual_pad::{VirtualPad, VirtualPadConfig};
EOF

commit "feat(pad): DualSense-like abs axis ranges for sticks/triggers" crates/pad/src/absinfo.rs crates/pad/src/lib.rs

echo "bootstrap part A done — continuing in same script..."
