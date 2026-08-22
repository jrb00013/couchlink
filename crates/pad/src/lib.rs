//! Pad stack: parse real DualSense / DualShock 4 / Xbox HID reports and inject
//! a virtual DualSense (Linux uhid/uinput) or Windows DualSense VHID / ViGEm pad.
//! Physical Xbox / DS4 / DualSense inputs normalize onto the same `PadFrame`.

pub mod absinfo;
pub mod dualsense;
pub mod feedback;
pub mod map_frame;
pub mod parse;
pub mod parse_ds4;
pub mod parse_steam;
pub mod parse_switch;
pub mod parse_xbox;
pub mod recognize;
pub mod sim;
pub mod steam_controller;
pub mod switch;
pub mod vhid_client;
pub mod vhid_proto;
pub mod virtual_pad;
pub mod xbox;

#[cfg(target_os = "linux")]
pub mod linux_uhid;

#[cfg(windows)]
pub mod windows_pad;

#[cfg(test)]
mod controller_tester;

pub use dualsense::{PID_DUALSENSE, PID_DUALSENSE_EDGE, SONY_VID};
pub use parse::parse_input_report;
pub use parse_ds4::parse_ds4_input_report;
pub use parse_steam::parse_steam_input_report;
pub use parse_switch::parse_switch_input_report;
pub use parse_xbox::parse_xbox_input_report;
pub use recognize::{
    classify, is_dualshock4, is_native_supported, is_supported_dualsense, is_supported_steam_controller,
    is_supported_switch, is_supported_xbox, parse_hid_id_line, product_label, ControllerFamily,
    XboxVariant, DUALSHOCK4_PIDS,
};
pub use sim::{
    decode_clpd, dualsense_usb_press, encode_clpd, simulate_dualsense_frame, simulate_steam_frame,
    simulate_switch_frame, simulate_xbox_frame, steam_neutral_report, steam_press,
    switch_neutral_report, switch_press, xbox_press, SimButton,
};
pub use steam_controller::{KNOWN_PIDS as STEAM_KNOWN_PIDS, VALVE_VID};
pub use switch::{KNOWN_PIDS as SWITCH_KNOWN_PIDS, NINTENDO_VID};
pub use virtual_pad::{VirtualPad, VirtualPadBackend, VirtualPadConfig};
pub use xbox::{KNOWN_PIDS as XBOX_KNOWN_PIDS, MICROSOFT_VID};
