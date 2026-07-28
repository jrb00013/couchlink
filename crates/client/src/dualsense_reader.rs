//! Read local DualSense via hidapi — dualsensekit enumeration methodology.

use anyhow::{bail, Context, Result};
use couchlink_pad::dualsense::{PID_DUALSENSE, PID_DUALSENSE_EDGE, SONY_VID};
use couchlink_pad::parse_input_report;
use couchlink_proto::PadFrame;
use hidapi::{HidApi, HidDevice};
use tracing::info;

pub struct DualSenseReader {
    device: HidDevice,
    seq: u32,
}

impl DualSenseReader {
    pub fn open_first() -> Result<Self> {
        let api = HidApi::new().context("hidapi init")?;
        let mut candidates: Vec<_> = api
            .device_list()
            .filter(|d| {
                d.vendor_id() == SONY_VID
                    && (d.product_id() == PID_DUALSENSE || d.product_id() == PID_DUALSENSE_EDGE)
            })
            .collect();
        if candidates.is_empty() {
            bail!("no DualSense found (pair it first — see dualsensekit playbook)");
        }
        // Prefer USB (interface >= 0) like dualsensekit Python wrapper
        candidates.sort_by_key(|d| if d.interface_number() >= 0 { 0 } else { 1 });
        let info = candidates[0];
        let device = info.open_device(&api).context("open DualSense")?;
        info!(
            "opened DualSense pid={:04x} interface={}",
            info.product_id(),
            info.interface_number()
        );
        Ok(Self { device, seq: 0 })
    }

    pub fn read_frame(&mut self) -> Result<Option<PadFrame>> {
        let mut buf = [0u8; 128];
        let n = match self.device.read_timeout(&mut buf, 4) {
            Ok(n) => n,
            Err(_) => return Ok(None),
        };
        if n == 0 {
            return Ok(None);
        }
        let mut frame = match parse_input_report(&buf[..n]) {
            Some(f) => f,
            None => return Ok(None),
        };
        self.seq = self.seq.wrapping_add(1);
        frame.seq = self.seq;
        Ok(Some(frame))
    }
}
