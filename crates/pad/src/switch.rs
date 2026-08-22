//! Constants for Nintendo Switch controllers (Pro Controller, Joy-Con),
//! read on the client side and normalized into the same wire `PadFrame` the
//! DualSense reader produces.
//!
//! Vendor/product ids follow `hid-nintendo` (`drivers/hid/hid-nintendo.c`):
//! the Pro Controller enumerates as `0x057E:0x2009` over USB and Bluetooth,
//! and each Joy-Con reports as a separate device (`0x2006` left, `0x2007`
//! right). The charging-grip PID is accepted too: it is the pair of Joy-Cons
//! presented as one HID device over USB.

pub const NINTENDO_VID: u16 = 0x057E;

/// Nintendo Switch Pro Controller.
pub const PID_SWITCH_PRO: u16 = 0x2009;
/// Joy-Con (L).
pub const PID_JOYCON_L: u16 = 0x2006;
/// Joy-Con (R).
pub const PID_JOYCON_R: u16 = 0x2007;
/// Joy-Con pair presented over a charging grip (USB).
pub const PID_JOYCON_CHARGE_GRIP: u16 = 0x2008;

/// Known Nintendo controller product ids we parse on the client hidraw path.
pub const KNOWN_PIDS: &[u16] = &[
    PID_SWITCH_PRO,
    PID_JOYCON_L,
    PID_JOYCON_R,
    PID_JOYCON_CHARGE_GRIP,
];

pub const PRODUCT_NAME: &str = "Nintendo Switch Pro Controller";

/// Per-controller display labels, keyed by pid.
pub fn label_for_pid(pid: u16) -> &'static str {
    match pid {
        PID_SWITCH_PRO => PRODUCT_NAME,
        PID_JOYCON_L => "Joy-Con (L)",
        PID_JOYCON_R => "Joy-Con (R)",
        PID_JOYCON_CHARGE_GRIP => "Joy-Con Pair (Charging Grip)",
        _ => PRODUCT_NAME,
    }
}
