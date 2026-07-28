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
