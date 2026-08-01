//! Pad stack: parse real DualSense / Xbox HID reports and inject a virtual
//! DualSense that announces itself as Bluetooth. Xbox input is normalized
//! onto the same DualSense-shaped `PadFrame`, so the virtual pad the host
//! creates and the emulator binds to never has to know which physical
//! controller produced it.

pub mod absinfo;
pub mod dualsense;
pub mod feedback;
pub mod parse;
pub mod parse_xbox;
pub mod recognize;
pub mod sim;
pub mod virtual_pad;
pub mod xbox;

#[cfg(test)]
mod controller_tester;

pub use dualsense::{PID_DUALSENSE, PID_DUALSENSE_EDGE, SONY_VID};
pub use parse::parse_input_report;
pub use parse_xbox::parse_xbox_input_report;
pub use recognize::{
    classify, is_dualshock4, is_native_supported, is_supported_dualsense, is_supported_xbox,
    parse_hid_id_line, product_label, ControllerFamily, XboxVariant,
};
pub use sim::{
    decode_clpd, dualsense_usb_press, encode_clpd, simulate_dualsense_frame, simulate_xbox_frame,
    xbox_press, SimButton,
};
pub use virtual_pad::{VirtualPad, VirtualPadConfig};
pub use xbox::{KNOWN_PIDS as XBOX_KNOWN_PIDS, MICROSOFT_VID};
