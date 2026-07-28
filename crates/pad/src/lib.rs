//! Pad stack: parse real DualSense HID reports (dualsensekit layouts) and
//! inject a virtual DualSense that announces itself as Bluetooth.

pub mod absinfo;
pub mod dualsense;
pub mod feedback;
pub mod parse;
pub mod virtual_pad;

pub use dualsense::{PID_DUALSENSE, PID_DUALSENSE_EDGE, SONY_VID};
pub use parse::parse_input_report;
pub use virtual_pad::{VirtualPad, VirtualPadConfig};
