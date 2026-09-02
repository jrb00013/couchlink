//! Constants for the Valve Steam Controller (classic V1), read on the client
//! side and normalized into the same wire `PadFrame` the DualSense reader
//! produces.
//!
//! Valve does not publish product ids; these come from the Linux kernel
//! `hid-steam` driver (`drivers/hid/hid-ids.h`) and sc-controller:
//! `0x28DE:0x1102` is the wireless dongle, `0x1105` the controller over
//! Bluetooth, and `0x1106` the wired / wireless-receiver presentation.
//! All three surface the same input protocol on their hidraw node.

pub const VALVE_VID: u16 = 0x28DE;

/// Steam Controller wireless dongle.
pub const PID_STEAM_CONTROLLER_DONGLE: u16 = 0x1102;
/// Steam Controller over Bluetooth.
pub const PID_STEAM_CONTROLLER_BT: u16 = 0x1105;
/// Steam Controller wired / via wireless receiver.
pub const PID_STEAM_CONTROLLER: u16 = 0x1106;

/// Known Steam Controller product ids.
pub const KNOWN_PIDS: &[u16] = &[
    PID_STEAM_CONTROLLER_DONGLE,
    PID_STEAM_CONTROLLER_BT,
    PID_STEAM_CONTROLLER,
];

pub const PRODUCT_NAME: &str = "Steam Controller";

/// Feature report commands (`ID_*`) used to take the controller out of
/// "lizard mode" (the built-in mouse/keyboard emulation it boots into) and
/// into gamepad mode. Mirrors `hid-steam` `steam_set_lizard_mode(false)` and
/// sc-controller.
pub const ID_CLEAR_DIGITAL_MAPPINGS: u8 = 0x81;
pub const ID_SET_SETTINGS_VALUES: u8 = 0x87;

/// Settings ids relevant to gamepad mode (see `hid-steam` SETTING_*).
pub const SETTING_LEFT_TRACKPAD_MODE: u8 = 8;
pub const SETTING_RIGHT_TRACKPAD_MODE: u8 = 9;
/// Trackpad mode "none" disables the absolute-mouse emulation.
pub const TRACKPAD_MODE_NONE: u16 = 7;
