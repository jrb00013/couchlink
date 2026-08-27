//! Parse Nintendo Switch controller HID input reports into `PadFrame`.
//!
//! In standard full report mode (`0x30`) — the mode the controller boots into
//! over USB and Bluetooth — both the Pro Controller and the Joy-Cons pack
//! everything into a 49-byte report:
//!
//! ```text
//!  offset  size  meaning
//!  0       1     report id 0x30
//!  1       1     timer
//!  2       1     battery / connection info
//!  3       3     buttons (u24, little-endian byte order: bits 0-23)
//!  6       3     left  stick, 12-bit X and Y packed
//!  9       3     right stick, 12-bit X and Y packed
//!  12      1     vibrator report byte
//! ```
//!
//! Button bit layout (`hid-nintendo` `JC_BTN_*`):
//! byte 3: Y=0x01 X=0x02 B=0x04 A=0x08 SR_R=0x10 SL_R=0x20 R=0x40 ZR=0x80
//! byte 4: Minus=0x01 Plus=0x02 RStick=0x04 LStick=0x08 Home=0x10 Capture=0x20
//! byte 5: Down=0x01 Up=0x02 Right=0x04 Left=0x08 SR_L=0x10 SL_L=0x20 L=0x40 ZL=0x80
//!
//! Each stick axis is a 12-bit value (neutral 0x800) packed across three
//! bytes: axis A = low byte + low nibble of the middle byte, axis B = high
//! nibble of the middle byte + high byte shifted up. Face buttons are remapped
//! by *position*, not label, so the wire frame lands correctly on a
//! DualSense-shaped virtual pad: X→SQUARE (left), A→CROSS (bottom),
//! B→CIRCLE (right), Y→TRIANGLE (top).

use couchlink_proto::pad_frame::buttons;
use couchlink_proto::PadFrame;

pub const SWITCH_REPORT_ID: u8 = 0x30;

/// Minimum report length (id + header through the vibrator byte).
const MIN_LEN: usize = 13;

pub fn parse_switch_input_report(raw: &[u8]) -> Option<PadFrame> {
    if raw.len() < MIN_LEN || raw[0] != SWITCH_REPORT_ID {
        return None;
    }

    let b3 = raw[3];
    let b4 = raw[4];
    let b5 = raw[5];

    let lx = packed_stick(raw[6], raw[7], raw[8]).0;
    let ly = packed_stick(raw[6], raw[7], raw[8]).1;
    let rx = packed_stick(raw[9], raw[10], raw[11]).0;
    let ry = packed_stick(raw[9], raw[10], raw[11]).1;

    let mut out = 0u32;
    // Face buttons, mapped by position onto the DualSense diamond.
    if b3 & 0x01 != 0 {
        out |= buttons::TRIANGLE; // Y (top)
    }
    if b3 & 0x02 != 0 {
        out |= buttons::SQUARE; // X (left)
    }
    if b3 & 0x04 != 0 {
        out |= buttons::CIRCLE; // B (right)
    }
    if b3 & 0x08 != 0 {
        out |= buttons::CROSS; // A (bottom)
    }
    if b3 & 0x40 != 0 {
        out |= buttons::R1; // R
    }
    if b3 & 0x80 != 0 {
        out |= buttons::R2; // ZR
    }
    if b5 & 0x40 != 0 {
        out |= buttons::L1; // L
    }
    if b5 & 0x80 != 0 {
        out |= buttons::L2; // ZL
    }
    if b4 & 0x01 != 0 {
        out |= buttons::CREATE; // Minus
    }
    if b4 & 0x02 != 0 {
        out |= buttons::OPTIONS; // Plus
    }
    if b4 & 0x04 != 0 {
        out |= buttons::R3; // right stick click
    }
    if b4 & 0x08 != 0 {
        out |= buttons::L3; // left stick click
    }
    if b4 & 0x10 != 0 {
        out |= buttons::PS; // Home
    }
    // Capture has no DualSense equivalent; it is deliberately unmapped.
    if b5 & 0x01 != 0 {
        out |= buttons::DPAD_DOWN;
    }
    if b5 & 0x02 != 0 {
        out |= buttons::DPAD_UP;
    }
    if b5 & 0x04 != 0 {
        out |= buttons::DPAD_RIGHT;
    }
    if b5 & 0x08 != 0 {
        out |= buttons::DPAD_LEFT;
    }

    Some(PadFrame {
        seq: 0,
        buttons: out,
        lx,
        ly,
        rx,
        ry,
        l2: 0,
        r2: 0,
        gx: 0,
        gy: 0,
        gz: 0,
        touch_active: 0,
        touch_x: 0,
        touch_y: 0,
        client_ts_ms: 0,
    })
}

/// Unpack two 12-bit axes from a three-byte packed group; returns (x, y).
///
/// The controller centers at 0x800. We shift right by 4 so the u8 wire values
/// land in 0..=255 with neutral at 128, exactly like the other readers.
fn packed_stick(b0: u8, b1: u8, b2: u8) -> (u8, u8) {
    let x = (u16::from(b0) | (u16::from(b1 & 0x0F) << 8)) >> 4;
    let y = ((u16::from(b1) >> 4) | (u16::from(b2) << 4)) >> 4;
    (x as u8, y as u8)
}

/// Pack two 12-bit values (test helper mirroring the controller layout):
/// b0 = X low byte, b1 low nibble = X high nibble, b1 high nibble = Y low
/// nibble, b2 = Y high byte.
fn pack_stick(x: u16, y: u16) -> [u8; 3] {
    [
        x as u8,
        ((x >> 8) as u8 & 0x0F) | ((y & 0x0F) as u8) << 4,
        (y >> 4) as u8,
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn neutral_report() -> Vec<u8> {
        let mut raw = vec![0u8; 49];
        raw[0] = SWITCH_REPORT_ID;
        // packed sticks centered at 0x800
        let left = pack_stick(0x0800, 0x0800);
        let right = pack_stick(0x0800, 0x0800);
        raw[6..9].copy_from_slice(&left);
        raw[9..12].copy_from_slice(&right);
        raw
    }

    #[test]
    fn parse_neutral() {
        let f = parse_switch_input_report(&neutral_report()).unwrap();
        assert_eq!(f.lx, 128);
        assert_eq!(f.ly, 128);
        assert_eq!(f.rx, 128);
        assert_eq!(f.ry, 128);
        assert_eq!(f.buttons, 0);
    }

    #[test]
    fn face_buttons_map_by_position() {
        let mut raw = neutral_report();
        raw[3] = 0x08; // A (bottom)
        let f = parse_switch_input_report(&raw).unwrap();
        assert!(f.buttons & buttons::CROSS != 0);

        let mut raw = neutral_report();
        raw[3] = 0x02; // X (left)
        let f = parse_switch_input_report(&raw).unwrap();
        assert!(f.buttons & buttons::SQUARE != 0);

        let mut raw = neutral_report();
        raw[3] = 0x04; // B (right)
        let f = parse_switch_input_report(&raw).unwrap();
        assert!(f.buttons & buttons::CIRCLE != 0);

        let mut raw = neutral_report();
        raw[3] = 0x01; // Y (top)
        let f = parse_switch_input_report(&raw).unwrap();
        assert!(f.buttons & buttons::TRIANGLE != 0);
    }

    #[test]
    fn shoulders_and_center_buttons() {
        let mut raw = neutral_report();
        raw[3] = 0xC0; // R + ZR
        let f = parse_switch_input_report(&raw).unwrap();
        assert!(f.buttons & buttons::R1 != 0);
        assert!(f.buttons & buttons::R2 != 0);

        let mut raw = neutral_report();
        raw[5] = 0xC0; // L + ZL
        let f = parse_switch_input_report(&raw).unwrap();
        assert!(f.buttons & buttons::L1 != 0);
        assert!(f.buttons & buttons::L2 != 0);

        let mut raw = neutral_report();
        raw[4] = 0x0F; // Minus + Plus + RStick + LStick
        let f = parse_switch_input_report(&raw).unwrap();
        assert!(f.buttons & buttons::CREATE != 0);
        assert!(f.buttons & buttons::OPTIONS != 0);
        assert!(f.buttons & buttons::R3 != 0);
        assert!(f.buttons & buttons::L3 != 0);
    }

    #[test]
    fn dpad_and_home() {
        let mut raw = neutral_report();
        raw[5] = 0x0F; // Down + Up + Right + Left
        let f = parse_switch_input_report(&raw).unwrap();
        assert!(f.buttons & buttons::DPAD_DOWN != 0);
        assert!(f.buttons & buttons::DPAD_UP != 0);
        assert!(f.buttons & buttons::DPAD_RIGHT != 0);
        assert!(f.buttons & buttons::DPAD_LEFT != 0);

        let mut raw = neutral_report();
        raw[4] = 0x10; // Home
        let f = parse_switch_input_report(&raw).unwrap();
        assert!(f.buttons & buttons::PS != 0);
    }

    #[test]
    fn rejects_wrong_report_id() {
        let mut raw = neutral_report();
        raw[0] = 0x21; // subcommand reply
        assert!(parse_switch_input_report(&raw).is_none());
    }

    #[test]
    fn stick_values_round_trip() {
        let mut raw = neutral_report();
        // Left stick fully left/up (0x000), right stick fully right/down (0xFFF)
        let left = pack_stick(0x000, 0x000);
        let right = pack_stick(0xFFF, 0xFFF);
        raw[6..9].copy_from_slice(&left);
        raw[9..12].copy_from_slice(&right);
        let f = parse_switch_input_report(&raw).unwrap();
        assert_eq!(f.lx, 0);
        assert_eq!(f.ly, 0);
        assert_eq!(f.rx, 255);
        assert_eq!(f.ry, 255);
    }
}
