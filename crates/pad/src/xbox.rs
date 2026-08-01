//! Constants for Microsoft Xbox controllers (Xbox One / Series X|S / Xbox
//! Wireless), read on the client side and normalized into the same wire
//! `PadFrame` the DualSense reader produces.

pub const MICROSOFT_VID: u16 = 0x045E;

/// Xbox One S controller (USB).
pub const PID_XBOX_ONE_S: u16 = 0x02FD;
/// Xbox One S controller (Bluetooth LE / firmware pairing).
pub const PID_XBOX_ONE_S_BT: u16 = 0x02E0;
/// Xbox Wireless Controller (Model 1708 and later, USB).
pub const PID_XBOX_WIRELESS: u16 = 0x02FF;
/// Xbox Series X|S controller (USB).
pub const PID_XBOX_SERIES: u16 = 0x0B12;
/// Xbox Series X|S controller (Bluetooth).
pub const PID_XBOX_SERIES_BT: u16 = 0x0B13;
/// Xbox Elite Wireless Controller Series 2.
pub const PID_XBOX_ELITE_2: u16 = 0x0B00;

/// Known Xbox controller product IDs, USB and Bluetooth pairings alike.
pub const KNOWN_PIDS: &[u16] = &[
    PID_XBOX_ONE_S,
    PID_XBOX_ONE_S_BT,
    PID_XBOX_WIRELESS,
    PID_XBOX_SERIES,
    PID_XBOX_SERIES_BT,
    PID_XBOX_ELITE_2,
];

pub const PRODUCT_NAME: &str = "Xbox Wireless Controller";
