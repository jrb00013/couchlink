//! Host battery status for the viewers' battery indicator.
//!
//! The host PC is a laptop. Around 20% Windows switches power plans and the stream hitches for a couple of seconds, which the
//! friends could not tell apart from a network problem. WSL exposes the Windows battery under `/sys/class/power_supply`
//! (`BAT*` type `Battery`, `AC*` type `Mains`), so this is two tiny file reads, no PowerShell, no polling thread.

use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Battery {
    /// Charge, 0-100.
    pub percent: u8,
    /// The battery reports it is charging (not merely plugged in at 100%).
    pub charging: bool,
    /// Mains power is connected.
    pub ac: bool,
}

fn read_trim(p: &Path) -> Option<String> {
    std::fs::read_to_string(p).ok().map(|s| s.trim().to_string())
}

/// Current host battery, or `None` on a machine without one (desktop) or when sysfs is unavailable.
pub fn read() -> Option<Battery> {
    read_from(Path::new("/sys/class/power_supply"))
}

pub fn read_from(dir: &Path) -> Option<Battery> {
    let mut percent = None;
    let mut charging = false;
    let mut ac = false;
    for entry in std::fs::read_dir(dir).ok()?.flatten() {
        let p = entry.path();
        match read_trim(&p.join("type")).as_deref() {
            Some("Battery") if percent.is_none() => {
                percent = read_trim(&p.join("capacity")).and_then(|c| c.parse::<u32>().ok()).map(|c| c.min(100) as u8);
                charging = read_trim(&p.join("status")).as_deref() == Some("Charging");
            }
            Some("Mains") | Some("USB") => {
                if read_trim(&p.join("online")).as_deref() == Some("1") {
                    ac = true;
                }
            }
            _ => {}
        }
    }
    percent.map(|percent| Battery { percent, charging, ac })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dir(name: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("cl-battery-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn supply(root: &Path, name: &str, files: &[(&str, &str)]) {
        let d = root.join(name);
        std::fs::create_dir_all(&d).unwrap();
        for (f, v) in files {
            std::fs::write(d.join(f), format!("{v}\n")).unwrap();
        }
    }

    #[test]
    fn reads_percent_charging_and_ac() {
        let d = dir("charging");
        supply(&d, "BAT1", &[("type", "Battery"), ("capacity", "31"), ("status", "Charging")]);
        supply(&d, "AC1", &[("type", "Mains"), ("online", "1")]);
        assert_eq!(read_from(&d), Some(Battery { percent: 31, charging: true, ac: true }));
    }

    #[test]
    fn on_battery_is_not_charging_and_not_ac() {
        let d = dir("discharging");
        supply(&d, "BAT1", &[("type", "Battery"), ("capacity", "19"), ("status", "Discharging")]);
        supply(&d, "AC1", &[("type", "Mains"), ("online", "0")]);
        assert_eq!(read_from(&d), Some(Battery { percent: 19, charging: false, ac: false }));
    }

    #[test]
    fn plugged_in_at_full_is_ac_but_not_charging() {
        let d = dir("full");
        supply(&d, "BAT1", &[("type", "Battery"), ("capacity", "100"), ("status", "Full")]);
        supply(&d, "AC1", &[("type", "Mains"), ("online", "1")]);
        assert_eq!(read_from(&d), Some(Battery { percent: 100, charging: false, ac: true }));
    }

    #[test]
    fn no_battery_means_none() {
        let d = dir("desktop");
        supply(&d, "AC1", &[("type", "Mains"), ("online", "1")]);
        assert_eq!(read_from(&d), None);
        assert_eq!(read_from(&d.join("missing")), None);
    }

    #[test]
    fn garbage_capacity_is_none_and_over_100_is_clamped() {
        let d = dir("garbage");
        supply(&d, "BAT1", &[("type", "Battery"), ("capacity", "n/a"), ("status", "Unknown")]);
        assert_eq!(read_from(&d), None);
        supply(&d, "BAT1", &[("type", "Battery"), ("capacity", "250"), ("status", "Unknown")]);
        assert_eq!(read_from(&d).map(|b| b.percent), Some(100));
    }
}
