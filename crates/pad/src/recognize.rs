//! Controller identity recognition — the same VID/PID rules the Linux
//! hidraw readers use when deciding an Xbox or DualSense is "ours".
//!
//! Kept pure (no filesystem) so unit tests can prove every supported Xbox
//! variant and DualSense / DualSense Edge product is accepted, and that
//! lookalikes (e.g. DualShock 4 on native hidraw) are rejected.

use crate::dualsense::{PID_DUALSENSE, PID_DUALSENSE_EDGE, SONY_VID};
use crate::xbox::{
    KNOWN_PIDS as XBOX_PIDS, MICROSOFT_VID, PID_XBOX_ELITE_2, PID_XBOX_ONE_S, PID_XBOX_ONE_S_BT,
    PID_XBOX_SERIES, PID_XBOX_SERIES_BT, PID_XBOX_WIRELESS, PRODUCT_NAME as XBOX_NAME,
};
use crate::dualsense::PRODUCT_NAME as DUALSENSE_NAME;

/// DualShock 4 (PS4) product IDs — recognized by the browser Gamepad API path,
/// but **not** by the native Linux hidraw DualSense reader (different HID layout).
pub const PID_DUALSHOCK4_V1: u16 = 0x05C4;
pub const PID_DUALSHOCK4_V2: u16 = 0x09CC;
pub const PID_DUALSHOCK4_DONGLE: u16 = 0x0BA0;

pub const DUALSHOCK4_PIDS: &[u16] = &[PID_DUALSHOCK4_V1, PID_DUALSHOCK4_V2, PID_DUALSHOCK4_DONGLE];

/// Which physical pad family a VID/PID pair maps to for native capture.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ControllerFamily {
    Xbox,
    DualSense,
    /// PS4 DualShock 4 — web Standard Gamepad only; native hidraw does not parse it.
    DualShock4,
    Unknown,
}

/// Named Xbox SKU we accept on the client hidraw path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum XboxVariant {
    OneSUsb,
    OneSBluetooth,
    Wireless1708,
    SeriesUsb,
    SeriesBluetooth,
    EliteSeries2,
}

impl XboxVariant {
    pub fn pid(self) -> u16 {
        match self {
            Self::OneSUsb => PID_XBOX_ONE_S,
            Self::OneSBluetooth => PID_XBOX_ONE_S_BT,
            Self::Wireless1708 => PID_XBOX_WIRELESS,
            Self::SeriesUsb => PID_XBOX_SERIES,
            Self::SeriesBluetooth => PID_XBOX_SERIES_BT,
            Self::EliteSeries2 => PID_XBOX_ELITE_2,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::OneSUsb => "Xbox One S (USB)",
            Self::OneSBluetooth => "Xbox One S (Bluetooth)",
            Self::Wireless1708 => "Xbox Wireless Controller (1708+)",
            Self::SeriesUsb => "Xbox Series X|S (USB)",
            Self::SeriesBluetooth => "Xbox Series X|S (Bluetooth)",
            Self::EliteSeries2 => "Xbox Elite Wireless Series 2",
        }
    }

    pub const ALL: &'static [XboxVariant] = &[
        Self::OneSUsb,
        Self::OneSBluetooth,
        Self::Wireless1708,
        Self::SeriesUsb,
        Self::SeriesBluetooth,
        Self::EliteSeries2,
    ];
}

pub fn is_supported_xbox(vid: u16, pid: u16) -> bool {
    vid == MICROSOFT_VID && XBOX_PIDS.contains(&pid)
}

pub fn is_supported_dualsense(vid: u16, pid: u16) -> bool {
    vid == SONY_VID && (pid == PID_DUALSENSE || pid == PID_DUALSENSE_EDGE)
}

pub fn is_dualshock4(vid: u16, pid: u16) -> bool {
    vid == SONY_VID && DUALSHOCK4_PIDS.contains(&pid)
}

/// Native hidraw capture accept list (Xbox + DualSense / Edge only).
pub fn is_native_supported(vid: u16, pid: u16) -> bool {
    is_supported_xbox(vid, pid) || is_supported_dualsense(vid, pid)
}

pub fn xbox_variant(pid: u16) -> Option<XboxVariant> {
    XboxVariant::ALL.iter().copied().find(|v| v.pid() == pid)
}

pub fn classify(vid: u16, pid: u16) -> ControllerFamily {
    if is_supported_xbox(vid, pid) {
        ControllerFamily::Xbox
    } else if is_supported_dualsense(vid, pid) {
        ControllerFamily::DualSense
    } else if is_dualshock4(vid, pid) {
        ControllerFamily::DualShock4
    } else {
        ControllerFamily::Unknown
    }
}

/// Human label for logs / tester UI.
pub fn product_label(vid: u16, pid: u16) -> Option<&'static str> {
    match classify(vid, pid) {
        ControllerFamily::Xbox => Some(xbox_variant(pid).map(|v| v.label()).unwrap_or(XBOX_NAME)),
        ControllerFamily::DualSense => {
            if pid == PID_DUALSENSE_EDGE {
                Some("DualSense Edge")
            } else {
                Some(DUALSENSE_NAME)
            }
        }
        ControllerFamily::DualShock4 => Some("DualShock 4"),
        ControllerFamily::Unknown => None,
    }
}

/// Parse a sysfs `HID_ID=BBBB:VVVVVVVV:PPPPPPPP` line into (bus, vid, pid).
pub fn parse_hid_id_line(line: &str) -> Option<(u16, u16, u16)> {
    let raw = line.trim().strip_prefix("HID_ID=")?;
    let parts: Vec<&str> = raw.split(':').collect();
    if parts.len() < 3 {
        return None;
    }
    let bus = u16::from_str_radix(parts[0], 16).ok()?;
    let vid = u16::from_str_radix(parts[1], 16).ok()?;
    let pid = u16::from_str_radix(parts[2], 16).ok()?;
    Some((bus, vid, pid))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_xbox_variant_is_recognized() {
        for v in XboxVariant::ALL {
            assert!(
                is_supported_xbox(MICROSOFT_VID, v.pid()),
                "{} (PID {:04X}) must be recognized",
                v.label(),
                v.pid()
            );
            assert_eq!(classify(MICROSOFT_VID, v.pid()), ControllerFamily::Xbox);
            assert_eq!(xbox_variant(v.pid()), Some(*v));
            assert!(product_label(MICROSOFT_VID, v.pid()).is_some());
        }
        assert_eq!(XboxVariant::ALL.len(), XBOX_PIDS.len());
    }

    #[test]
    fn dualsense_and_edge_are_recognized() {
        assert!(is_supported_dualsense(SONY_VID, PID_DUALSENSE));
        assert!(is_supported_dualsense(SONY_VID, PID_DUALSENSE_EDGE));
        assert_eq!(classify(SONY_VID, PID_DUALSENSE), ControllerFamily::DualSense);
        assert_eq!(
            classify(SONY_VID, PID_DUALSENSE_EDGE),
            ControllerFamily::DualSense
        );
    }

    #[test]
    fn dualshock4_classified_but_not_native() {
        for &pid in DUALSHOCK4_PIDS {
            assert_eq!(classify(SONY_VID, pid), ControllerFamily::DualShock4);
            assert!(!is_native_supported(SONY_VID, pid));
            assert!(!is_supported_dualsense(SONY_VID, pid));
        }
    }

    #[test]
    fn rejects_unknown_microsoft_and_sony_pids() {
        assert!(!is_supported_xbox(MICROSOFT_VID, 0x028E)); // Xbox 360
        assert!(!is_native_supported(SONY_VID, 0x0268)); // Sixaxis
        assert_eq!(classify(0x1234, 0x5678), ControllerFamily::Unknown);
    }

    #[test]
    fn parses_sysfs_hid_id() {
        let (bus, vid, pid) =
            parse_hid_id_line("HID_ID=0003:0000045E:00000B12").unwrap();
        assert_eq!(bus, 0x0003);
        assert_eq!(vid, MICROSOFT_VID);
        assert_eq!(pid, PID_XBOX_SERIES);
        assert!(is_native_supported(vid, pid));
    }
}
