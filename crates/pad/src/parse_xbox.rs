//! Parse Xbox One / Series controller HID input reports into `PadFrame`.
//!
//! Layout follows the generic HID gamepad report Xbox controllers expose over
//! `hidraw` (the same one xpadneo / xow document for Bluetooth, and that USB
//! controllers fall back to without the proprietary xpad driver bound):
//! report id 0x01, then LX/LY/RX/RY as little-endian i16, LT/RT as
//! little-endian u16 (0-1023), a D-pad hat nibble, then two button bytes.
//!
//! Face buttons are remapped by *position*, not label, so the wire frame
//! (SQUARE/CROSS/CIRCLE/TRIANGLE — a DualSense diamond) lands correctly on a
//! DualSense-shaped virtual pad: X→SQUARE (left), A→CROSS (bottom),
//! B→CIRCLE (right), Y→TRIANGLE (top).

use couchlink_proto::pad_frame::buttons;
use couchlink_proto::PadFrame;

pub const XBOX_REPORT_ID: u8 = 0x01;

/// Minimum body length (after the report id byte) this layout needs.
const MIN_BODY_LEN: usize = 13;

pub fn parse_xbox_input_report(raw: &[u8]) -> Option<PadFrame> {
    if raw.len() < 2 || raw[0] != XBOX_REPORT_ID {
        return None;
    }
    let body = &raw[1..];
    if body.len() < MIN_BODY_LEN {
        return None;
    }

    let lx = axis_to_u8(i16::from_le_bytes([body[0], body[1]]));
    let ly = axis_to_u8(i16::from_le_bytes([body[2], body[3]]));
    let rx = axis_to_u8(i16::from_le_bytes([body[4], body[5]]));
    let ry = axis_to_u8(i16::from_le_bytes([body[6], body[7]]));
    let lt = u16::from_le_bytes([body[8], body[9]]);
    let rt = u16::from_le_bytes([body[10], body[11]]);
    let l2 = trigger_to_u8(lt);
    let r2 = trigger_to_u8(rt);

    let hat = body[12] & 0x0F;
    let button_lo = body.get(13).copied().unwrap_or(0);
    let button_hi = body.get(14).copied().unwrap_or(0);

    let mut out = 0u32;
    // Face buttons, mapped by position onto the DualSense diamond.
    if button_lo & 0x01 != 0 {
        out |= buttons::CROSS; // A (bottom)
    }
    if button_lo & 0x02 != 0 {
        out |= buttons::CIRCLE; // B (right)
    }
    if button_lo & 0x04 != 0 {
        out |= buttons::SQUARE; // X (left)
    }
    if button_lo & 0x08 != 0 {
        out |= buttons::TRIANGLE; // Y (top)
    }
    if button_lo & 0x10 != 0 {
        out |= buttons::L1; // LB
    }
    if button_lo & 0x20 != 0 {
        out |= buttons::R1; // RB
    }
    if button_lo & 0x40 != 0 {
        out |= buttons::PS; // Xbox / Guide button
    }
    if button_lo & 0x80 != 0 {
        out |= buttons::CREATE; // View
    }
    if button_hi & 0x01 != 0 {
        out |= buttons::OPTIONS; // Menu
    }
    if button_hi & 0x02 != 0 {
        out |= buttons::L3; // left stick click
    }
    if button_hi & 0x04 != 0 {
        out |= buttons::R3; // right stick click
    }
    // Analog triggers also register as digital presses past a threshold, so
    // the host's L2/R2 button bits stay in sync with the analog value.
    if l2 > 0x10 {
        out |= buttons::L2;
    }
    if r2 > 0x10 {
        out |= buttons::R2;
    }

    match hat {
        0 => out |= buttons::DPAD_UP,
        1 => out |= buttons::DPAD_UP | buttons::DPAD_RIGHT,
        2 => out |= buttons::DPAD_RIGHT,
        3 => out |= buttons::DPAD_DOWN | buttons::DPAD_RIGHT,
        4 => out |= buttons::DPAD_DOWN,
        5 => out |= buttons::DPAD_DOWN | buttons::DPAD_LEFT,
        6 => out |= buttons::DPAD_LEFT,
        7 => out |= buttons::DPAD_UP | buttons::DPAD_LEFT,
        _ => {} // 8 = released
    }

    Some(PadFrame {
        seq: 0,
        buttons: out,
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

/// i16 full-range axis (-32768..=32767, center 0) → u8 (0..=255, center 128).
fn axis_to_u8(v: i16) -> u8 {
    (((v as i32) + 32768) >> 8) as u8
}

/// 10-bit trigger (0..=1023) → u8 (0..=255).
fn trigger_to_u8(v: u16) -> u8 {
    (v.min(1023) >> 2) as u8
}

#[cfg(test)]
mod tests {
    use super::*;

    fn neutral_report() -> Vec<u8> {
        let mut raw = vec![0u8; 16];
        raw[0] = XBOX_REPORT_ID;
        // centered sticks
        raw[1..3].copy_from_slice(&0i16.to_le_bytes());
        raw[3..5].copy_from_slice(&0i16.to_le_bytes());
        raw[5..7].copy_from_slice(&0i16.to_le_bytes());
        raw[7..9].copy_from_slice(&0i16.to_le_bytes());
        raw[13] = 8; // hat released
        raw
    }

    #[test]
    fn parse_neutral() {
        let f = parse_xbox_input_report(&neutral_report()).unwrap();
        assert_eq!(f.lx, 128);
        assert_eq!(f.ly, 128);
        assert_eq!(f.rx, 128);
        assert_eq!(f.ry, 128);
        assert_eq!(f.buttons, 0);
    }

    #[test]
    fn detects_a_button() {
        let mut raw = neutral_report();
        raw[14] = 0x01; // button_lo bit0 = A
        let f = parse_xbox_input_report(&raw).unwrap();
        assert!(f.buttons & buttons::CROSS != 0);
    }

    #[test]
    fn detects_dpad_up() {
        let mut raw = neutral_report();
        raw[13] = 0; // hat = up
        let f = parse_xbox_input_report(&raw).unwrap();
        assert!(f.buttons & buttons::DPAD_UP != 0);
    }

    #[test]
    fn rejects_wrong_report_id() {
        let mut raw = neutral_report();
        raw[0] = 0x20;
        assert!(parse_xbox_input_report(&raw).is_none());
    }

    #[test]
    fn full_trigger_pull_sets_digital_bit() {
        let mut raw = neutral_report();
        raw[9..11].copy_from_slice(&1023u16.to_le_bytes());
        let f = parse_xbox_input_report(&raw).unwrap();
        assert_eq!(f.l2, 255);
        assert!(f.buttons & buttons::L2 != 0);
    }
}
