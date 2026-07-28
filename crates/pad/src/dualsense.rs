//! Constants and report IDs — aligned with dualsensekit PROTOCOL.md.

pub const SONY_VID: u16 = 0x054C;
pub const PID_DUALSENSE: u16 = 0x0CE6;
pub const PID_DUALSENSE_EDGE: u16 = 0x0DF2;

pub const FEATURE_BT_CONTROL: u8 = 0x08;
pub const FEATURE_PAIRING: u8 = 0x09;
pub const FEATURE_BT_PAIRING: u8 = 0x0A;
pub const FEATURE_FIRMWARE: u8 = 0x20;

pub const INPUT_USB: u8 = 0x01;
pub const INPUT_BT: u8 = 0x31;
pub const OUTPUT_USB: u8 = 0x02;

pub const USB_REPORT_LEN: usize = 64;
pub const BT_REPORT_LEN: usize = 78;

/// Product string emulators expect.
pub const PRODUCT_NAME: &str = "DualSense Wireless Controller";
