//! Simulated HID input reports for controller testing without hardware.
//!
//! Build Xbox or DualSense report bytes, parse them into `PadFrame`, and
//! (optionally) encode CLPD for the host path — same pipeline the real
//! client readers and host injector use.

use couchlink_proto::pad_frame::{buttons, PadCodecError, PAD_FRAME_LEN};
use couchlink_proto::PadFrame;

use crate::dualsense::{INPUT_BT, INPUT_USB};
use crate::parse::parse_input_report;
use crate::parse_steam::{parse_steam_input_report, STEAM_REPORT_ID};
use crate::parse_switch::{parse_switch_input_report, SWITCH_REPORT_ID};
use crate::parse_xbox::{parse_xbox_input_report, XBOX_REPORT_ID};

/// Which face / shoulder / system button to press in a simulated report.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SimButton {
    // Position-based (Xbox A/B/X/Y and DualSense diamond share these names
    // after remapping).
    Cross,
    Circle,
    Square,
    Triangle,
    L1,
    R1,
    L2,
    R2,
    Create,
    Options,
    L3,
    R3,
    Ps,
    Touch,
    Mute,
    DpadUp,
    DpadDown,
    DpadLeft,
    DpadRight,
}

impl SimButton {
    pub fn pad_bit(self) -> u32 {
        match self {
            Self::Cross => buttons::CROSS,
            Self::Circle => buttons::CIRCLE,
            Self::Square => buttons::SQUARE,
            Self::Triangle => buttons::TRIANGLE,
            Self::L1 => buttons::L1,
            Self::R1 => buttons::R1,
            Self::L2 => buttons::L2,
            Self::R2 => buttons::R2,
            Self::Create => buttons::CREATE,
            Self::Options => buttons::OPTIONS,
            Self::L3 => buttons::L3,
            Self::R3 => buttons::R3,
            Self::Ps => buttons::PS,
            Self::Touch => buttons::TOUCH,
            Self::Mute => buttons::MUTE,
            Self::DpadUp => buttons::DPAD_UP,
            Self::DpadDown => buttons::DPAD_DOWN,
            Self::DpadLeft => buttons::DPAD_LEFT,
            Self::DpadRight => buttons::DPAD_RIGHT,
        }
    }
}

/// Neutral Xbox HID report (sticks centered, hat released, no buttons).
pub fn xbox_neutral_report() -> Vec<u8> {
    let mut raw = vec![0u8; 16];
    raw[0] = XBOX_REPORT_ID;
    for pair in [1usize, 3, 5, 7] {
        raw[pair..pair + 2].copy_from_slice(&0i16.to_le_bytes());
    }
    raw[13] = 8; // hat released
    raw
}

/// Neutral DualSense USB input report.
pub fn dualsense_usb_neutral_report() -> Vec<u8> {
    let mut raw = vec![0u8; 64];
    raw[0] = INPUT_USB;
    raw[1] = 128;
    raw[2] = 128;
    raw[3] = 128;
    raw[4] = 128;
    raw[8] = 0x08; // dpad released (nibble 8)
    raw
}

/// Neutral DualSense Bluetooth input report (`0x31` + `0x01` tag + USB-like body).
pub fn dualsense_bt_neutral_report() -> Vec<u8> {
    let mut raw = vec![0u8; 78];
    raw[0] = INPUT_BT;
    raw[1] = 0x01;
    raw[2] = 128;
    raw[3] = 128;
    raw[4] = 128;
    raw[5] = 128;
    raw[9] = 0x08;
    raw
}

/// Press one Xbox button (or set hat) on a neutral report. Analog L2/R2 use a
/// full 10-bit pull so the digital bit also latches.
pub fn xbox_press(btn: SimButton) -> Vec<u8> {
    let mut raw = xbox_neutral_report();
    match btn {
        SimButton::Cross => raw[14] |= 0x01,
        SimButton::Circle => raw[14] |= 0x02,
        SimButton::Square => raw[14] |= 0x04,
        SimButton::Triangle => raw[14] |= 0x08,
        SimButton::L1 => raw[14] |= 0x10,
        SimButton::R1 => raw[14] |= 0x20,
        SimButton::Ps => raw[14] |= 0x40,
        SimButton::Create => raw[14] |= 0x80,
        SimButton::Options => raw[15] |= 0x01,
        SimButton::L3 => raw[15] |= 0x02,
        SimButton::R3 => raw[15] |= 0x04,
        SimButton::L2 => raw[9..11].copy_from_slice(&1023u16.to_le_bytes()),
        SimButton::R2 => raw[11..13].copy_from_slice(&1023u16.to_le_bytes()),
        SimButton::DpadUp => raw[13] = 0,
        SimButton::DpadDown => raw[13] = 4,
        SimButton::DpadLeft => raw[13] = 6,
        SimButton::DpadRight => raw[13] = 2,
        SimButton::Touch | SimButton::Mute => {}
    }
    raw
}

/// Press one DualSense USB button / dpad / digital trigger bit.
pub fn dualsense_usb_press(btn: SimButton) -> Vec<u8> {
    let mut raw = dualsense_usb_neutral_report();
    // body starts at raw[1]
    match btn {
        SimButton::Square => raw[8] |= 0x10,
        SimButton::Cross => raw[8] |= 0x20,
        SimButton::Circle => raw[8] |= 0x40,
        SimButton::Triangle => raw[8] |= 0x80,
        SimButton::L1 => raw[9] |= 0x01,
        SimButton::R1 => raw[9] |= 0x02,
        SimButton::L2 => {
            raw[9] |= 0x04;
            raw[5] = 255;
        }
        SimButton::R2 => {
            raw[9] |= 0x08;
            raw[6] = 255;
        }
        SimButton::Create => raw[9] |= 0x10,
        SimButton::Options => raw[9] |= 0x20,
        SimButton::L3 => raw[9] |= 0x40,
        SimButton::R3 => raw[9] |= 0x80,
        SimButton::Ps => raw[10] |= 0x01,
        SimButton::Touch => raw[10] |= 0x02,
        SimButton::Mute => raw[10] |= 0x04,
        SimButton::DpadUp => raw[8] = (raw[8] & 0xF0) | 0x00,
        SimButton::DpadDown => raw[8] = (raw[8] & 0xF0) | 0x04,
        SimButton::DpadLeft => raw[8] = (raw[8] & 0xF0) | 0x06,
        SimButton::DpadRight => raw[8] = (raw[8] & 0xF0) | 0x02,
    }
    raw
}

/// Xbox sticks as full-range i16; DualSense sticks as u8 (0..=255, 128 center).
pub fn xbox_with_sticks(lx: i16, ly: i16, rx: i16, ry: i16) -> Vec<u8> {
    let mut raw = xbox_neutral_report();
    raw[1..3].copy_from_slice(&lx.to_le_bytes());
    raw[3..5].copy_from_slice(&ly.to_le_bytes());
    raw[5..7].copy_from_slice(&rx.to_le_bytes());
    raw[7..9].copy_from_slice(&ry.to_le_bytes());
    raw
}

pub fn dualsense_usb_with_sticks(lx: u8, ly: u8, rx: u8, ry: u8) -> Vec<u8> {
    let mut raw = dualsense_usb_neutral_report();
    raw[1] = lx;
    raw[2] = ly;
    raw[3] = rx;
    raw[4] = ry;
    raw
}

/// Client-side: Xbox HID bytes → normalized `PadFrame`.
pub fn simulate_xbox_frame(raw: &[u8]) -> Option<PadFrame> {
    parse_xbox_input_report(raw)
}

/// Neutral Switch standard-report (0x30) with both sticks centered at 0x800.
pub fn switch_neutral_report() -> Vec<u8> {
    let mut raw = vec![0u8; 49];
    raw[0] = SWITCH_REPORT_ID;
    for off in [6usize, 9] {
        // pack 0x800 into the 3-byte group
        raw[off] = 0x00;
        raw[off + 1] = 0x08;
        raw[off + 2] = 0x80;
    }
    raw
}

/// Press one Switch button on a neutral report. Triggers ZL/ZR latch their
/// digital bits; sticks stay centered.
pub fn switch_press(btn: SimButton) -> Vec<u8> {
    let mut raw = switch_neutral_report();
    match btn {
        SimButton::Cross => raw[3] |= 0x08, // A (bottom)
        SimButton::Circle => raw[3] |= 0x04, // B (right)
        SimButton::Square => raw[3] |= 0x02, // X (left)
        SimButton::Triangle => raw[3] |= 0x01, // Y (top)
        SimButton::R1 => raw[3] |= 0x40, // R
        SimButton::R2 => raw[3] |= 0x80, // ZR
        SimButton::L1 => raw[5] |= 0x40, // L
        SimButton::L2 => raw[5] |= 0x80, // ZL
        SimButton::Create => raw[4] |= 0x01, // Minus
        SimButton::Options => raw[4] |= 0x02, // Plus
        SimButton::R3 => raw[4] |= 0x04, // RStick
        SimButton::L3 => raw[4] |= 0x08, // LStick
        SimButton::Ps => raw[4] |= 0x10, // Home
        SimButton::DpadUp => raw[5] |= 0x02,
        SimButton::DpadDown => raw[5] |= 0x01,
        SimButton::DpadLeft => raw[5] |= 0x08,
        SimButton::DpadRight => raw[5] |= 0x04,
        SimButton::Touch | SimButton::Mute => {}
    }
    raw
}

/// Client-side: Switch HID bytes → normalized `PadFrame`.
pub fn simulate_switch_frame(raw: &[u8]) -> Option<PadFrame> {
    parse_switch_input_report(raw)
}

/// Neutral Steam Controller report (0x01) with pads centered and triggers up.
pub fn steam_neutral_report() -> Vec<u8> {
    let mut raw = vec![0u8; 64];
    raw[0] = STEAM_REPORT_ID;
    for pair in [16usize, 18, 20, 22] {
        raw[pair..pair + 2].copy_from_slice(&0i16.to_le_bytes());
    }
    raw
}

/// Press one Steam Controller button on a neutral report. L2/R2 pull the
/// analog trigger fully so the digital bit latches too.
pub fn steam_press(btn: SimButton) -> Vec<u8> {
    let mut raw = steam_neutral_report();
    match btn {
        SimButton::Cross => raw[8] |= 0x80, // A (bottom)
        SimButton::Circle => raw[8] |= 0x20, // B (right)
        SimButton::Square => raw[8] |= 0x40, // X (left)
        SimButton::Triangle => raw[8] |= 0x10, // Y (top)
        SimButton::L1 => raw[8] |= 0x08, // left shoulder
        SimButton::R1 => raw[8] |= 0x04, // right shoulder
        SimButton::L2 => {
            raw[8] |= 0x02; // ZL digital
            raw[11] = 255;
        }
        SimButton::R2 => {
            raw[8] |= 0x01; // ZR digital
            raw[12] = 255;
        }
        SimButton::Create => raw[9] |= 0x10, // menu left
        SimButton::Options => raw[9] |= 0x40, // menu right
        SimButton::Ps => raw[9] |= 0x20, // Steam logo
        SimButton::L3 => raw[10] |= 0x40, // joystick click (left pad)
        SimButton::R3 => raw[10] |= 0x04, // right pad click
        SimButton::DpadUp => raw[9] |= 0x01,
        SimButton::DpadDown => raw[9] |= 0x08,
        SimButton::DpadLeft => raw[9] |= 0x04,
        SimButton::DpadRight => raw[9] |= 0x02,
        SimButton::Touch | SimButton::Mute => {}
    }
    raw
}

/// Client-side: Steam Controller HID bytes → normalized `PadFrame`.
pub fn simulate_steam_frame(raw: &[u8]) -> Option<PadFrame> {
    parse_steam_input_report(raw)
}

/// Client-side: DualSense HID bytes → normalized `PadFrame`.
pub fn simulate_dualsense_frame(raw: &[u8]) -> Option<PadFrame> {
    parse_input_report(raw)
}

/// Encode a frame the way the client sends it on the `pad` DataChannel.
pub fn encode_clpd(frame: &PadFrame) -> Vec<u8> {
    let mut out = bytes::BytesMut::with_capacity(PAD_FRAME_LEN);
    frame.encode(&mut out);
    out.to_vec()
}

/// Host-side: CLPD bytes → `PadFrame` (what `apply_pad_bytes` decodes before uinput).
pub fn decode_clpd(data: &[u8]) -> Result<PadFrame, PadCodecError> {
    PadFrame::decode(data)
}

/// Buttons exercised on both Xbox and DualSense native paths.
pub const SHARED_BUTTONS: &[SimButton] = &[
    SimButton::Cross,
    SimButton::Circle,
    SimButton::Square,
    SimButton::Triangle,
    SimButton::L1,
    SimButton::R1,
    SimButton::L2,
    SimButton::R2,
    SimButton::Create,
    SimButton::Options,
    SimButton::L3,
    SimButton::R3,
    SimButton::Ps,
    SimButton::DpadUp,
    SimButton::DpadDown,
    SimButton::DpadLeft,
    SimButton::DpadRight,
];

/// DualSense-only extras (no Xbox HID equivalent in our report layout).
pub const DUALSENSE_ONLY_BUTTONS: &[SimButton] = &[SimButton::Touch, SimButton::Mute];
