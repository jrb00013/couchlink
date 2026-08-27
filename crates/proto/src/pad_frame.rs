//! Binary pad protocol (`CLPD`) — lower latency than JSON for ~250 Hz DualSense state.
//! Layout inspired by dualsensekit USB/BT input reports, normalized for the wire.

use bytes::{Buf, BufMut, BytesMut};
use thiserror::Error;

/// WebRTC DataChannel label for pad traffic.
pub const PAD_CHANNEL: &str = "pad";
/// ASCII magic.
pub const PAD_MAGIC: &[u8; 4] = b"CLPD";
pub const PAD_VERSION: u8 = 1;
pub const PAD_VERSION_V2: u8 = 2;
/// magic(4)+ver(1)+seq(4)+buttons(4)+sticks/triggers(6)+gyro(6)+touch(5)+reserved(1)
pub const PAD_FRAME_LEN: usize = 31;
/// v2 adds client_ts_ms (u32 LE) after the v1 body.
pub const PAD_FRAME_LEN_V2: usize = 35;

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
    /// Browser `performance.now()` at send (ms, u32 wrap ok). 0 = v1 / unknown.
    pub client_ts_ms: u32,
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
        out.reserve(PAD_FRAME_LEN_V2);
        out.put_slice(PAD_MAGIC);
        out.put_u8(PAD_VERSION_V2);
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
        out.put_u8(0);
        out.put_u32_le(self.client_ts_ms);
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
        let frame = PadFrame {
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
            client_ts_ms: 0,
        };
        match ver {
            PAD_VERSION => {
                let _reserved = buf.get_u8();
                Ok(frame)
            }
            PAD_VERSION_V2 => {
                if buf.len() < 5 {
                    return Err(PadCodecError::Short);
                }
                let _reserved = buf.get_u8();
                let client_ts_ms = buf.get_u32_le();
                Ok(PadFrame {
                    client_ts_ms,
                    ..frame
                })
            }
            v => Err(PadCodecError::BadVersion(v)),
        }
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

#[cfg(test)]
mod tests {
    use super::*;
    use bytes::BytesMut;

    #[test]
    fn v1_31_byte_frame_decodes_with_zero_client_ts() {
        let mut legacy = BytesMut::new();
        legacy.put_slice(PAD_MAGIC);
        legacy.put_u8(PAD_VERSION);
        let f = PadFrame {
            seq: 7,
            buttons: buttons::CROSS,
            ..PadFrame::neutral()
        };
        legacy.put_u32_le(f.seq);
        legacy.put_u32_le(f.buttons);
        legacy.put_u8(f.lx);
        legacy.put_u8(f.ly);
        legacy.put_u8(f.rx);
        legacy.put_u8(f.ry);
        legacy.put_u8(f.l2);
        legacy.put_u8(f.r2);
        legacy.put_i16_le(f.gx);
        legacy.put_i16_le(f.gy);
        legacy.put_i16_le(f.gz);
        legacy.put_u8(f.touch_active);
        legacy.put_u16_le(f.touch_x);
        legacy.put_u16_le(f.touch_y);
        legacy.put_u8(0);
        assert_eq!(legacy.len(), PAD_FRAME_LEN);
        let back = PadFrame::decode(&legacy).unwrap();
        assert_eq!(back.seq, 7);
        assert_eq!(back.client_ts_ms, 0);
    }

    #[test]
    fn v2_round_trips_client_ts_ms() {
        let f = PadFrame {
            seq: 9,
            client_ts_ms: 1_234_567,
            ..PadFrame::neutral()
        };
        let mut buf = BytesMut::new();
        f.encode(&mut buf);
        assert_eq!(buf.len(), PAD_FRAME_LEN_V2);
        let back = PadFrame::decode(&buf).unwrap();
        assert_eq!(back.client_ts_ms, 1_234_567);
        assert_eq!(back.seq, 9);
    }
}
