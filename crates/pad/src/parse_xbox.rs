//! Parse Xbox One / Series controller HID input reports into `PadFrame`.
//!
//! Layout is the standard gamepad report Xbox controllers expose over
//! `hidraw` (the same one SDL's HIDAPI driver and xpadneo document): a report
//! id byte followed by left/right sticks as little-endian u16 centered on
//! `0x8000`, both analog triggers as little-endian u16 (10-bit value,
//! `0..=1023`), a D-pad hat nibble (`1..=8` clockwise, `0` released), then the
//! button bytes. Two button-byte layouts exist depending on controller
//! firmware and are told apart by packet size:
//!
//! * 16-byte packets (original Xbox One S firmware): the third button byte
//!   holds only the stick clicks (`0x01` L3, `0x02` R3); the guide button is
//!   delivered in a separate report.
//! * 17+ byte packets (One S firmware 5.x, Series X|S, Elite 2): the third
//!   button byte holds View/Menu/Guide/stick clicks and the fourth byte the
//!   Share button.
//!
//! Face buttons are remapped by *position*, not label, so the wire frame
//! (SQUARE/CROSS/CIRCLE/TRIANGLE — a DualSense diamond) lands correctly on a
//! DualSense-shaped virtual pad: X→SQUARE (left), A→CROSS (bottom),
//! B→CIRCLE (right), Y→TRIANGLE (top). The guide button travels in report
//! id `0x02` packets on older firmware and inside the state packet on newer
//! firmware; both are accepted here.

use couchlink_proto::pad_frame::buttons;
use couchlink_proto::PadFrame;

/// Main state packet report id (sticks, triggers, hat, buttons).
pub const XBOX_REPORT_ID: u8 = 0x01;
/// Guide (Xbox button) report id, used by older firmware that keeps it out of
/// the state packet.
pub const XBOX_GUIDE_REPORT_ID: u8 = 0x02;

/// Minimum body length (after the report id byte) for a state packet: four
/// stick u16s, two trigger u16s, hat byte, first two button bytes.
const MIN_BODY_LEN: usize = 15;

/// Parse a raw hidraw report from an Xbox One / Series controller.
///
/// Returns a frame with sticks centered on 128 and all buttons released for a
/// neutral state packet, the guide button set for a guide packet, and `None`
/// for any other report id or undersized packet.
pub fn parse_xbox_input_report(raw: &[u8]) -> Option<PadFrame> {
    if raw.len() < 2 {
        return None;
    }
    match raw[0] {
        XBOX_REPORT_ID => parse_state_packet(&raw[1..]),
        XBOX_GUIDE_REPORT_ID => {
            // One byte: body[0] bit 0 = guide (Xbox button) pressed.
            let mut out = neutral_frame();
            if raw[1] & 0x01 != 0 {
                out.buttons |= buttons::PS;
            }
            Some(out)
        }
        _ => None,
    }
}

fn parse_state_packet(body: &[u8]) -> Option<PadFrame> {
    if body.len() < MIN_BODY_LEN {
        return None;
    }

    let lx = stick_to_u8(u16::from_le_bytes([body[0], body[1]]));
    let ly = stick_to_u8(u16::from_le_bytes([body[2], body[3]]));
    let rx = stick_to_u8(u16::from_le_bytes([body[4], body[5]]));
    let ry = stick_to_u8(u16::from_le_bytes([body[6], body[7]]));
    let lt = u16::from_le_bytes([body[8], body[9]]);
    let rt = u16::from_le_bytes([body[10], body[11]]);
    let l2 = trigger_to_u8(lt);
    let r2 = trigger_to_u8(rt);

    let hat = body[12] & 0x0F;
    let button_a = body[13];
    let button_b = body[14];
    let button_c = body.get(15).copied().unwrap_or(0);

    let mut out = neutral_frame();
    out.lx = lx;
    out.ly = ly;
    out.rx = rx;
    out.ry = ry;
    out.l2 = l2;
    out.r2 = r2;

    // Face + shoulder buttons, mapped by position onto the DualSense diamond.
    if button_a & 0x01 != 0 {
        out.buttons |= buttons::CROSS; // A (bottom)
    }
    if button_a & 0x02 != 0 {
        out.buttons |= buttons::CIRCLE; // B (right)
    }
    if button_a & 0x04 != 0 {
        out.buttons |= buttons::SQUARE; // X (left)
    }
    if button_a & 0x08 != 0 {
        out.buttons |= buttons::TRIANGLE; // Y (top)
    }
    if button_a & 0x10 != 0 {
        out.buttons |= buttons::L1; // LB
    }
    if button_a & 0x20 != 0 {
        out.buttons |= buttons::R1; // RB
    }
    if button_a & 0x40 != 0 {
        out.buttons |= buttons::CREATE; // View
    }
    if button_a & 0x80 != 0 {
        out.buttons |= buttons::OPTIONS; // Menu
    }

    // The second button byte means different things depending on firmware:
    // a 15-byte body is the original Xbox One S packet with only stick clicks
    // (guide arrives separately), anything longer is the unified layout.
    if body.len() == 15 {
        if button_b & 0x01 != 0 {
            out.buttons |= buttons::L3;
        }
        if button_b & 0x02 != 0 {
            out.buttons |= buttons::R3;
        }
    } else {
        if button_b & 0x04 != 0 {
            out.buttons |= buttons::CREATE; // View (newer firmwares)
        }
        if button_b & 0x08 != 0 {
            out.buttons |= buttons::OPTIONS; // Menu
        }
        if button_b & 0x10 != 0 {
            out.buttons |= buttons::PS; // Guide / Xbox button
        }
        if button_b & 0x20 != 0 {
            out.buttons |= buttons::L3;
        }
        if button_b & 0x40 != 0 {
            out.buttons |= buttons::R3;
        }
        if button_c & 0x01 != 0 {
            out.buttons |= buttons::CREATE; // Series Share (or One S View)
        }
    }

    // Analog triggers also register as digital presses past a threshold, so
    // the host's L2/R2 button bits stay in sync with the analog value.
    if l2 > 0x10 {
        out.buttons |= buttons::L2;
    }
    if r2 > 0x10 {
        out.buttons |= buttons::R2;
    }

    match hat {
        1 => out.buttons |= buttons::DPAD_UP,
        2 => out.buttons |= buttons::DPAD_UP | buttons::DPAD_RIGHT,
        3 => out.buttons |= buttons::DPAD_RIGHT,
        4 => out.buttons |= buttons::DPAD_DOWN | buttons::DPAD_RIGHT,
        5 => out.buttons |= buttons::DPAD_DOWN,
        6 => out.buttons |= buttons::DPAD_DOWN | buttons::DPAD_LEFT,
        7 => out.buttons |= buttons::DPAD_LEFT,
        8 => out.buttons |= buttons::DPAD_UP | buttons::DPAD_LEFT,
        _ => {} // 0 = released
    }

    Some(out)
}

fn neutral_frame() -> PadFrame {
    PadFrame {
        seq: 0,
        buttons: 0,
        lx: 128,
        ly: 128,
        rx: 128,
        ry: 128,
        l2: 0,
        r2: 0,
        gx: 0,
        gy: 0,
        gz: 0,
        touch_active: 0,
        touch_x: 0,
        touch_y: 0,
    }
}

/// u16 stick centered on `0x8000` (0..=65535) → u8 (0..=255, center 128).
fn stick_to_u8(v: u16) -> u8 {
    (v >> 8) as u8
}

/// 10-bit trigger (0..=1023) → u8 (0..=255).
fn trigger_to_u8(v: u16) -> u8 {
    (v.min(1023) >> 2) as u8
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Unified 17-byte report (report id + 16 body bytes): sticks centered on
    /// `0x8000`, triggers released, hat released (0), no buttons.
    fn unified_report() -> Vec<u8> {
        let mut raw = vec![0u8; 17];
        raw[0] = XBOX_REPORT_ID;
        for pair in [1usize, 3, 5, 7] {
            raw[pair..pair + 2].copy_from_slice(&0x8000u16.to_le_bytes());
        }
        raw
    }

    /// Original Xbox One S 16-byte report (report id + 15 body bytes).
    fn legacy_report() -> Vec<u8> {
        let mut raw = vec![0u8; 16];
        raw[0] = XBOX_REPORT_ID;
        for pair in [1usize, 3, 5, 7] {
            raw[pair..pair + 2].copy_from_slice(&0x8000u16.to_le_bytes());
        }
        raw
    }

    #[test]
    fn parse_neutral() {
        let f = parse_xbox_input_report(&unified_report()).unwrap();
        assert_eq!(f.lx, 128);
        assert_eq!(f.ly, 128);
        assert_eq!(f.rx, 128);
        assert_eq!(f.ry, 128);
        assert_eq!(f.buttons, 0);
    }

    #[test]
    fn sticks_are_u16_centered_on_0x8000() {
        let mut raw = unified_report();
        raw[1..3].copy_from_slice(&0xFFFFu16.to_le_bytes()); // LX full right
        raw[5..7].copy_from_slice(&0u16.to_le_bytes()); // RX full left
        let f = parse_xbox_input_report(&raw).unwrap();
        assert_eq!(f.lx, 255);
        assert_eq!(f.rx, 0);
    }

    #[test]
    fn detects_a_button() {
        let mut raw = unified_report();
        raw[14] = 0x01; // button_a bit0 = A
        let f = parse_xbox_input_report(&raw).unwrap();
        assert!(f.buttons & buttons::CROSS != 0);
    }

    #[test]
    fn maps_system_buttons_on_unified_layout() {
        let mut raw = unified_report();
        raw[14] = 0x40; // View
        let f = parse_xbox_input_report(&raw).unwrap();
        assert!(f.buttons & buttons::CREATE != 0);

        let mut raw = unified_report();
        raw[14] = 0x80; // Menu
        let f = parse_xbox_input_report(&raw).unwrap();
        assert!(f.buttons & buttons::OPTIONS != 0);

        let mut raw = unified_report();
        raw[15] = 0x10; // Guide
        let f = parse_xbox_input_report(&raw).unwrap();
        assert!(f.buttons & buttons::PS != 0);

        let mut raw = unified_report();
        raw[15] = 0x20; // L3
        let f = parse_xbox_input_report(&raw).unwrap();
        assert!(f.buttons & buttons::L3 != 0);

        let mut raw = unified_report();
        raw[15] = 0x40; // R3
        let f = parse_xbox_input_report(&raw).unwrap();
        assert!(f.buttons & buttons::R3 != 0);

        let mut raw = unified_report();
        raw[16] = 0x01; // Series Share
        let f = parse_xbox_input_report(&raw).unwrap();
        assert!(f.buttons & buttons::CREATE != 0);
    }

    #[test]
    fn legacy_packet_maps_stick_clicks() {
        let mut raw = legacy_report();
        raw[15] = 0x01; // L3
        let f = parse_xbox_input_report(&raw).unwrap();
        assert!(f.buttons & buttons::L3 != 0);

        let mut raw = legacy_report();
        raw[15] = 0x02; // R3
        let f = parse_xbox_input_report(&raw).unwrap();
        assert!(f.buttons & buttons::R3 != 0);
    }

    #[test]
    fn detects_dpad_up() {
        let mut raw = unified_report();
        raw[13] = 1; // hat = up
        let f = parse_xbox_input_report(&raw).unwrap();
        assert!(f.buttons & buttons::DPAD_UP != 0);
    }

    #[test]
    fn hat_zero_is_released() {
        let f = parse_xbox_input_report(&unified_report()).unwrap();
        assert_eq!(f.buttons & buttons::DPAD_UP, 0);
    }

    #[test]
    fn guide_packet_sets_ps_button() {
        let raw = [XBOX_GUIDE_REPORT_ID, 0x01];
        let f = parse_xbox_input_report(&raw).unwrap();
        assert!(f.buttons & buttons::PS != 0);
        assert_eq!(f.lx, 128);

        let raw = [XBOX_GUIDE_REPORT_ID, 0x00];
        let f = parse_xbox_input_report(&raw).unwrap();
        assert_eq!(f.buttons, 0);
    }

    #[test]
    fn rejects_wrong_report_id() {
        let mut raw = unified_report();
        raw[0] = 0x20;
        assert!(parse_xbox_input_report(&raw).is_none());
    }

    #[test]
    fn full_trigger_pull_sets_digital_bit() {
        let mut raw = unified_report();
        raw[9..11].copy_from_slice(&1023u16.to_le_bytes());
        let f = parse_xbox_input_report(&raw).unwrap();
        assert_eq!(f.l2, 255);
        assert!(f.buttons & buttons::L2 != 0);
    }
}
