//! Map host rumble / lightbar / adaptive-trigger feedback toward the player's
//! real DualSense (USB output report `0x02`).

use couchlink_proto::PadFeedback;

/// USB DualSense output report size (report id + common + reserved), matching
/// `DS_OUTPUT_REPORT_USB_SIZE` in hid-playstation.
pub const DS_USB_OUTPUT_LEN: usize = 63;

/// Bit flags in `valid_flag0` (common[0]).
pub const FLAG0_COMPATIBLE_VIBRATION: u8 = 1 << 0;
pub const FLAG0_HAPTICS_SELECT: u8 = 1 << 1;
pub const FLAG0_RIGHT_TRIGGER: u8 = 1 << 2;
pub const FLAG0_LEFT_TRIGGER: u8 = 1 << 3;

/// Bit flags in `valid_flag1` (common[1]).
pub const FLAG1_MIC_MUTE_LED: u8 = 1 << 0;
pub const FLAG1_POWER_SAVE: u8 = 1 << 1;
pub const FLAG1_LIGHTBAR: u8 = 1 << 2;
pub const FLAG1_RELEASE_LEDS: u8 = 1 << 3;
pub const FLAG1_PLAYER_INDICATOR: u8 = 1 << 4;

pub fn encode_feedback_json(fb: &PadFeedback) -> Result<String, serde_json::Error> {
    serde_json::to_string(fb)
}

/// Build a DualSense USB output report from structured feedback.
///
/// `RawOutput` copies the provided bytes (truncated/padded to [`DS_USB_OUTPUT_LEN`]).
/// Other variants set the relevant valid flags and fields; unspecified fields stay 0.
pub fn build_usb_output_report(fb: &PadFeedback) -> [u8; DS_USB_OUTPUT_LEN] {
    let mut buf = [0u8; DS_USB_OUTPUT_LEN];
    buf[0] = 0x02;
    match fb {
        PadFeedback::RawOutput { report } => {
            let n = report.len().min(DS_USB_OUTPUT_LEN);
            buf[..n].copy_from_slice(&report[..n]);
            if n == 0 || buf[0] == 0 {
                buf[0] = 0x02;
            }
        }
        PadFeedback::Rumble { large, small } => {
            // valid_flag0: enable motors (compat vibration + haptics select)
            buf[1] = FLAG0_COMPATIBLE_VIBRATION | FLAG0_HAPTICS_SELECT;
            buf[3] = *small; // motor_right (high-freq)
            buf[4] = *large; // motor_left (low-freq)
        }
        PadFeedback::Lightbar { r, g, b } => {
            buf[2] = FLAG1_LIGHTBAR;
            buf[45] = *r;
            buf[46] = *g;
            buf[47] = *b;
        }
        PadFeedback::PlayerLed { mask } => {
            buf[2] = FLAG1_PLAYER_INDICATOR;
            buf[44] = *mask;
        }
        PadFeedback::AdaptiveTriggers {
            left_mode,
            left_params,
            right_mode,
            right_params,
        } => {
            buf[1] = FLAG0_RIGHT_TRIGGER | FLAG0_LEFT_TRIGGER;
            // Right trigger block starts at USB index 11 (common offset 10)
            buf[11] = *right_mode;
            copy_params(&mut buf[12..22], right_params);
            // Left trigger block starts at USB index 22
            buf[22] = *left_mode;
            copy_params(&mut buf[23..33], left_params);
        }
    }
    buf
}

fn copy_params(dst: &mut [u8], src: &[u8]) {
    let n = src.len().min(dst.len());
    dst[..n].copy_from_slice(&src[..n]);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rumble_sets_motors_and_flags() {
        let fb = PadFeedback::Rumble {
            large: 200,
            small: 40,
        };
        let r = build_usb_output_report(&fb);
        assert_eq!(r[0], 0x02);
        assert_eq!(r[1] & FLAG0_COMPATIBLE_VIBRATION, FLAG0_COMPATIBLE_VIBRATION);
        assert_eq!(r[3], 40);
        assert_eq!(r[4], 200);
    }

    #[test]
    fn adaptive_triggers_land_at_known_offsets() {
        let fb = PadFeedback::AdaptiveTriggers {
            left_mode: 0x01,
            left_params: vec![0, 200],
            right_mode: 0x02,
            right_params: vec![10, 40, 180],
        };
        let r = build_usb_output_report(&fb);
        assert_eq!(r[11], 0x02);
        assert_eq!(&r[12..15], &[10, 40, 180]);
        assert_eq!(r[22], 0x01);
        assert_eq!(&r[23..25], &[0, 200]);
        assert_eq!(r[1] & FLAG0_LEFT_TRIGGER, FLAG0_LEFT_TRIGGER);
        assert_eq!(r[1] & FLAG0_RIGHT_TRIGGER, FLAG0_RIGHT_TRIGGER);
    }

    #[test]
    fn raw_output_passthrough() {
        let mut report = vec![0u8; 20];
        report[0] = 0x02;
        report[1] = 0xFF;
        report[3] = 1;
        let fb = PadFeedback::RawOutput { report: report.clone() };
        let r = build_usb_output_report(&fb);
        assert_eq!(&r[..20], report.as_slice());
    }

    #[test]
    fn json_roundtrip_adaptive() {
        let fb = PadFeedback::AdaptiveTriggers {
            left_mode: 6,
            left_params: vec![1, 2, 3],
            right_mode: 0,
            right_params: vec![],
        };
        let s = encode_feedback_json(&fb).unwrap();
        let back: PadFeedback = serde_json::from_str(&s).unwrap();
        assert_eq!(back, fb);
    }
}
