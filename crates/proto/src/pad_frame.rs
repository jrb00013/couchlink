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
/// magic(4)+ver(1)+seq(4)+buttons(4)+sticks/triggers(6)+gyro(6)+touch(5)+reserved(1)
pub const PAD_FRAME_LEN: usize = 31;

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

/// Host → player haptic / lightbar / adaptive-trigger feedback (JSON on same channel).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum PadFeedback {
    Rumble { large: u8, small: u8 },
    Lightbar { r: u8, g: u8, b: u8 },
    PlayerLed { mask: u8 },
    /// DualSense adaptive trigger effect blocks (USB output offsets 11 / 22).
    AdaptiveTriggers {
        left_mode: u8,
        /// Up to 10 effect parameters (padded/truncated to 10 on pack).
        #[serde(default)]
        left_params: Vec<u8>,
        right_mode: u8,
        #[serde(default)]
        right_params: Vec<u8>,
    },
    /// Full DualSense USB output report (report id `0x02` + common payload).
    RawOutput {
        /// Raw HID bytes including report id.
        report: Vec<u8>,
    },
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
