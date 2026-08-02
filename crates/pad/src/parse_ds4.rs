//! DualShock 4 (PS4) USB/BT input report → `PadFrame`.
//! Layout follows hid-sony / dualsensekit community offsets for report `0x01`.

use couchlink_proto::pad_frame::buttons;
use couchlink_proto::PadFrame;

const DS4_USB: u8 = 0x01;
const DS4_BT: u8 = 0x11;

pub fn parse_ds4_input_report(raw: &[u8]) -> Option<PadFrame> {
    if raw.is_empty() {
        return None;
    }
    let body = match raw[0] {
        DS4_USB => raw.get(1..)?,
        DS4_BT => {
            // BT: often 0x11 then counter then USB-like payload
            if raw.len() < 4 {
                return None;
            }
            &raw[3..]
        }
        _ if raw.len() >= 10 => raw,
        _ => return None,
    };
    if body.len() < 10 {
        return None;
    }

    let lx = body[0];
    let ly = body[1];
    let rx = body[2];
    let ry = body[3];
    let buttons_l = body[4];
    let buttons_h = body[5];
    let buttons_x = body.get(6).copied().unwrap_or(0);
    let l2 = body.get(7).copied().unwrap_or(0);
    let r2 = body.get(8).copied().unwrap_or(0);

    let dpad = buttons_l & 0x0F;
    let mut buttons = 0u32;
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
        buttons |= buttons::CREATE; // Share
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
    if buttons_x & 0x01 != 0 {
        buttons |= buttons::PS;
    }
    if buttons_x & 0x02 != 0 {
        buttons |= buttons::TOUCH;
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
    fn ds4_usb_cross() {
        let mut raw = [0u8; 32];
        raw[0] = DS4_USB;
        raw[5] = 0x20; // Cross in buttons_l high nibble (offset body[4] = raw[5])
        // body[4] is raw[5]
        let f = parse_ds4_input_report(&raw).unwrap();
        assert!(f.buttons & buttons::CROSS != 0);
    }
}
