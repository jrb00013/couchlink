//! Pad stack: parse real DualSense HID reports (dualsensekit layouts) and
//! inject a virtual DualSense that announces itself as Bluetooth.

pub mod dualsense;
pub mod parse;
pub mod virtual_pad;

pub use dualsense::{SONY_VID, PID_DUALSENSE, PID_DUALSENSE_EDGE};
pub use parse::parse_input_report;
pub use virtual_pad::{VirtualPad, VirtualPadConfig};
