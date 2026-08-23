//! Parse Steam Controller (V1) HID input reports into `PadFrame`.
//!
//! Layout follows `hid-steam` `steam_do_input_event` (report `ID_CONTROLLER_STATE`
//! = 0x01, a 64-byte payload). The offsets below are the raw report offsets:
//!
//! ```text
//!  offset  size  meaning
//!  8       u8    buttons 1: bit0 ZR  bit1 ZL  bit2 R  bit3 L
//!                bit4 Y bit5 B bit6 X bit7 A
//!  9       u8    buttons 2: bit0 DUp bit1 DRight bit2 DLeft bit3 DDown
//!                bit4 MenuL bit5 Steam bit6 MenuR bit7 GRIPL
//!  10      u8    buttons 3: bit0 GRIPR bit1 lpad-click bit2 rpad-click
//!                bit3 lpad-touched bit4 rpad-touched bit6 joystick-click
//!  11      u8    left trigger analog (0-255)
//!  12      u8    right trigger analog (0-255)
//!  16-17   s16   left  pad / joystick X
//!  18-19   s16   left  pad / joystick Y (negated on the wire)
//!  20-21   s16   right pad X
//!  22-23   s16   right pad Y (negated on the wire)
//! ```
//!
//! The V1 has no physical joysticks: its left touchpad doubles as the left
//! stick, and pressing the touchpads produces the L3 / R3 clicks. Face buttons
//! are remapped by *position* onto the DualSense diamond (A→CROSS, B→CIRCLE,
//! X→SQUARE, Y→TRIANGLE).
//!
//! Lizard mode: without Steam / `sc-controller` / a `hid-steam` gamepad-mode
//! switch the controller boots in mouse+keyboard emulation. `hid-steam` strips
//! lizard mode when a client opens the input device; opening this reader's
//! hidraw node does the same via the Steam client interface, and the reader
//! additionally sends the feature reports needed to put the pad into gamepad
//! mode (see `steam_controller.rs`).

use couchlink_proto::pad_frame::buttons;
use couchlink_proto::PadFrame;

pub const STEAM_REPORT_ID: u8 = 0x01;

/// Minimum report length (id byte + through right-pad Y).
const MIN_LEN: usize = 24;

pub fn parse_steam_input_report(raw: &[u8]) -> Option<PadFrame> {
    if raw.len() < MIN_LEN || raw[0] != STEAM_REPORT_ID {
        return None;
    }

    let b8 = raw[8];
    let b9 = raw[9];
    let b10 = raw[10];

    let lx = axis_to_u8(i16::from_le_bytes([raw[16], raw[17]]));
    let ly = axis_to_u8(i16::from_le_bytes([raw[18], raw[19]]).wrapping_neg());
    let rx = axis_to_u8(i16::from_le_bytes([raw[20], raw[21]]));
    let ry = axis_to_u8(i16::from_le_bytes([raw[22], raw[23]]).wrapping_neg());

    let l2 = raw[11];
    let r2 = raw[12];

    let mut out = 0u32;
    // Face buttons, mapped by position onto the DualSense diamond.
    if b8 & 0x80 != 0 {
        out |= buttons::CROSS; // A (bottom)
    }
    if b8 & 0x20 != 0 {
        out |= buttons::CIRCLE; // B (right)
    }
    if b8 & 0x40 != 0 {
        out |= buttons::SQUARE; // X (left)
    }
    if b8 & 0x10 != 0 {
        out |= buttons::TRIANGLE; // Y (top)
    }
    if b8 & 0x08 != 0 {
        out |= buttons::L1; // left shoulder
    }
    if b8 & 0x04 != 0 {
        out |= buttons::R1; // right shoulder
    }
    if b9 & 0x10 != 0 {
        out |= buttons::CREATE; // menu left
    }
    if b9 & 0x20 != 0 {
        out |= buttons::PS; // Steam logo
    }
    if b9 & 0x40 != 0 {
        out |= buttons::OPTIONS; // menu right
    }
    if b9 & 0x01 != 0 {
        out |= buttons::DPAD_UP;
    }
    if b9 & 0x02 != 0 {
        out |= buttons::DPAD_RIGHT;
    }
    if b9 & 0x04 != 0 {
        out |= buttons::DPAD_LEFT;
    }
    if b9 & 0x08 != 0 {
        out |= buttons::DPAD_DOWN;
    }
    if b10 & 0x04 != 0 {
        out |= buttons::R3; // right pad click
    }
    if b10 & 0x40 != 0 {
        out |= buttons::L3; // joystick click (left pad, joystick-style)
    }
    if l2 > 0x10 || b8 & 0x02 != 0 {
        out |= buttons::L2; // left trigger (analog + digital ZL)
    }
    if r2 > 0x10 || b8 & 0x01 != 0 {
        out |= buttons::R2; // right trigger (analog + digital ZR)
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
        client_ts_ms: 0,
    })
}

/// i16 full-range axis (-32768..=32767, center 0) → u8 (0..=255, center 128).
fn axis_to_u8(v: i16) -> u8 {
    (((v as i32) + 32768) >> 8) as u8
}

#[cfg(test)]
mod tests {
    use super::*;

    fn neutral_report() -> Vec<u8> {
        let mut raw = vec![0u8; 64];
        raw[0] = STEAM_REPORT_ID;
        // centered pads / sticks
        raw[16..18].copy_from_slice(&0i16.to_le_bytes());
        raw[18..20].copy_from_slice(&0i16.to_le_bytes());
        raw[20..22].copy_from_slice(&0i16.to_le_bytes());
        raw[22..24].copy_from_slice(&0i16.to_le_bytes());
        raw
    }

    #[test]
    fn parse_neutral() {
        let f = parse_steam_input_report(&neutral_report()).unwrap();
        assert_eq!(f.lx, 128);
        assert_eq!(f.ly, 128);
        assert_eq!(f.rx, 128);
        assert_eq!(f.ry, 128);
        assert_eq!(f.l2, 0);
        assert_eq!(f.r2, 0);
        assert_eq!(f.buttons, 0);
    }

    #[test]
    fn face_buttons_map_by_position() {
        for (bit, expected) in [
            (0x80, buttons::CROSS),  // A (bottom)
            (0x20, buttons::CIRCLE), // B (right)
            (0x40, buttons::SQUARE), // X (left)
            (0x10, buttons::TRIANGLE),
        ] {
            let mut raw = neutral_report();
            raw[8] = bit;
            let f = parse_steam_input_report(&raw).unwrap();
            assert_ne!(f.buttons & expected, 0, "bit {bit:#04x} → {expected:#x}");
        }
    }

    #[test]
    fn shoulders_triggers_and_menu() {
        let mut raw = neutral_report();
        raw[8] = 0x0F; // ZR + ZL + R + L
        let f = parse_steam_input_report(&raw).unwrap();
        assert!(f.buttons & buttons::R1 != 0);
        assert!(f.buttons & buttons::L1 != 0);
        assert!(f.buttons & buttons::R2 != 0);
        assert!(f.buttons & buttons::L2 != 0);

        let mut raw = neutral_report();
        raw[9] = 0x70; // MenuL + Steam + MenuR
        let f = parse_steam_input_report(&raw).unwrap();
        assert!(f.buttons & buttons::CREATE != 0);
        assert!(f.buttons & buttons::PS != 0);
        assert!(f.buttons & buttons::OPTIONS != 0);
    }

    #[test]
    fn pad_clicks_and_dpad() {
        let mut raw = neutral_report();
        raw[10] = 0x44; // rpad-click + joystick-click
        let f = parse_steam_input_report(&raw).unwrap();
        assert!(f.buttons & buttons::R3 != 0);
        assert!(f.buttons & buttons::L3 != 0);

        let mut raw = neutral_report();
        raw[9] = 0x0F; // dpad all directions
        let f = parse_steam_input_report(&raw).unwrap();
        assert!(f.buttons & buttons::DPAD_UP != 0);
        assert!(f.buttons & buttons::DPAD_RIGHT != 0);
        assert!(f.buttons & buttons::DPAD_LEFT != 0);
        assert!(f.buttons & buttons::DPAD_DOWN != 0);
    }

    #[test]
    fn full_trigger_pull_sets_digital_bit() {
        let mut raw = neutral_report();
        raw[12] = 0xFF;
        let f = parse_steam_input_report(&raw).unwrap();
        assert_eq!(f.r2, 255);
        assert!(f.buttons & buttons::R2 != 0);
    }

    #[test]
    fn rejects_wrong_report_id() {
        let mut raw = neutral_report();
        raw[0] = 0x05; // secondary / debug report
        assert!(parse_steam_input_report(&raw).is_none());
    }
}
