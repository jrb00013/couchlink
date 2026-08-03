//! Map `PadFrame` onto backend-specific virtual controller states.

use couchlink_proto::pad_frame::buttons;
use couchlink_proto::PadFrame;

/// DualSense USB input report (id `0x01`, 64 bytes) for custom VHID injection.
pub fn pad_frame_to_dualsense_usb_report(frame: &PadFrame) -> [u8; 64] {
    let mut r = [0u8; 64];
    r[0] = 0x01;
    r[1] = frame.lx;
    r[2] = frame.ly;
    r[3] = frame.rx;
    r[4] = frame.ry;
    r[5] = frame.l2;
    r[6] = frame.r2;
    // buttons_l: dpad nibble + face
    let mut bl = dpad_nibble(frame.buttons);
    if frame.buttons & buttons::SQUARE != 0 {
        bl |= 0x10;
    }
    if frame.buttons & buttons::CROSS != 0 {
        bl |= 0x20;
    }
    if frame.buttons & buttons::CIRCLE != 0 {
        bl |= 0x40;
    }
    if frame.buttons & buttons::TRIANGLE != 0 {
        bl |= 0x80;
    }
    r[8] = bl;
    let mut bh = 0u8;
    if frame.buttons & buttons::L1 != 0 {
        bh |= 0x01;
    }
    if frame.buttons & buttons::R1 != 0 {
        bh |= 0x02;
    }
    if frame.buttons & buttons::L2 != 0 {
        bh |= 0x04;
    }
    if frame.buttons & buttons::R2 != 0 {
        bh |= 0x08;
    }
    if frame.buttons & buttons::CREATE != 0 {
        bh |= 0x10;
    }
    if frame.buttons & buttons::OPTIONS != 0 {
        bh |= 0x20;
    }
    if frame.buttons & buttons::L3 != 0 {
        bh |= 0x40;
    }
    if frame.buttons & buttons::R3 != 0 {
        bh |= 0x80;
    }
    r[9] = bh;
    let mut be = 0u8;
    if frame.buttons & buttons::PS != 0 {
        be |= 0x01;
    }
    if frame.buttons & buttons::TOUCH != 0 {
        be |= 0x02;
    }
    if frame.buttons & buttons::MUTE != 0 {
        be |= 0x04;
    }
    r[10] = be;
    r
}

fn dpad_nibble(b: u32) -> u8 {
    let u = b & buttons::DPAD_UP != 0;
    let d = b & buttons::DPAD_DOWN != 0;
    let l = b & buttons::DPAD_LEFT != 0;
    let r = b & buttons::DPAD_RIGHT != 0;
    match (u, d, l, r) {
        (true, false, false, false) => 0,
        (true, false, false, true) => 1,
        (false, false, false, true) => 2,
        (false, true, false, true) => 3,
        (false, true, false, false) => 4,
        (false, true, true, false) => 5,
        (false, false, true, false) => 6,
        (true, false, true, false) => 7,
        _ => 8, // neutral
    }
}

/// XInput-style stick: -32768..32767 from DualSense 0..255 (128 center).
pub fn stick_u8_to_i16(v: u8) -> i16 {
    let centered = v as i32 - 128;
    (centered * 256).clamp(-32768, 32767) as i16
}

#[cfg(test)]
mod tests {
    use super::*;
    use couchlink_proto::pad_frame::buttons;

    #[test]
    fn dualsense_report_marks_cross() {
        let mut f = PadFrame::default();
        f.buttons = buttons::CROSS;
        f.lx = 128;
        f.ly = 128;
        let r = pad_frame_to_dualsense_usb_report(&f);
        assert_eq!(r[0], 0x01);
        assert!(r[8] & 0x20 != 0);
    }

    #[test]
    fn stick_center_is_zero() {
        assert_eq!(stick_u8_to_i16(128), 0);
    }

    #[test]
    fn dualsense_report_roundtrips_through_parser() {
        let mut f = PadFrame::neutral();
        f.buttons = buttons::CROSS | buttons::L1 | buttons::DPAD_UP;
        f.lx = 10;
        f.ly = 20;
        f.rx = 30;
        f.ry = 40;
        f.l2 = 50;
        f.r2 = 60;
        let report = pad_frame_to_dualsense_usb_report(&f);
        let back = crate::parse_input_report(&report).unwrap();
        assert_eq!(back.buttons & buttons::CROSS, buttons::CROSS);
        assert_eq!(back.buttons & buttons::L1, buttons::L1);
        assert_eq!(back.buttons & buttons::DPAD_UP, buttons::DPAD_UP);
        assert_eq!(back.lx, 10);
        assert_eq!(back.ly, 20);
        assert_eq!(back.rx, 30);
        assert_eq!(back.ry, 40);
        assert_eq!(back.l2, 50);
        assert_eq!(back.r2, 60);
    }
}
