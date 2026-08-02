//! Virtual pad backends for the companion.

use std::sync::Arc;

use anyhow::Result;
use couchlink_pad::vhid_proto::DS_USB_INPUT_LEN;

use crate::session::OutputHub;

pub trait PadBackend: Send {
    fn apply_ds_report(&mut self, report: &[u8; DS_USB_INPUT_LEN]) -> Result<()>;
}

pub fn create(
    kind: crate::BackendKind,
    hub: OutputHub,
) -> Result<Arc<std::sync::Mutex<dyn PadBackend>>> {
    match kind {
        crate::BackendKind::Ds4 => Ok(Arc::new(std::sync::Mutex::new(VigemDs4::create()?))),
        crate::BackendKind::Xbox360 => {
            Ok(Arc::new(std::sync::Mutex::new(VigemXbox::create(hub)?)))
        }
    }
}

struct VigemDs4 {
    target: vigem_client::DualShock4Wired<vigem_client::Client>,
}

impl VigemDs4 {
    fn create() -> Result<Self> {
        use anyhow::Context;
        let client = vigem_client::Client::connect()
            .context("ViGEmBus connect — install https://github.com/nefarius/ViGEmBus/releases")?;
        let id = vigem_client::TargetId::DUALSHOCK4_WIRED;
        let mut target = vigem_client::DualShock4Wired::new(client, id);
        target.plugin().context("ViGEm DS4 plugin")?;
        target.wait_ready().context("ViGEm DS4 wait_ready")?;
        tracing::info!("ViGEm DualShock 4 plugged (P2)");
        Ok(Self { target })
    }
}

impl PadBackend for VigemDs4 {
    fn apply_ds_report(&mut self, report: &[u8; DS_USB_INPUT_LEN]) -> Result<()> {
        use anyhow::Context;
        let lx = report[1];
        let ly = report[2];
        let rx = report[3];
        let ry = report[4];
        let l2 = report[5];
        let r2 = report[6];
        let bl = report[8];
        let bh = report[9];
        let be = report[10];
        let mut btn = (bl & 0x0F) as u16;
        if btn > 8 {
            btn = 8;
        }
        btn |= (bl & 0xF0) as u16;
        btn |= (bh as u16) << 8;
        let special = be & 0x03;
        let ds4 = vigem_client::DS4Report {
            thumb_lx: lx,
            thumb_ly: ly,
            thumb_rx: rx,
            thumb_ry: ry,
            buttons: btn,
            special,
            trigger_l: l2,
            trigger_r: r2,
        };
        self.target.update(&ds4).context("ViGEm DS4 update")?;
        Ok(())
    }
}

struct VigemXbox {
    target: vigem_client::Xbox360Wired<vigem_client::Client>,
    /// Keep notification thread alive for the lifetime of the backend.
    _notification: Option<std::thread::JoinHandle<()>>,
}

impl VigemXbox {
    fn create(hub: OutputHub) -> Result<Self> {
        use anyhow::Context;
        use couchlink_pad::feedback::build_usb_output_report;
        use couchlink_proto::PadFeedback;

        let client = vigem_client::Client::connect().context("ViGEmBus connect")?;
        let id = vigem_client::TargetId::XBOX360_WIRED;
        let mut target = vigem_client::Xbox360Wired::new(client, id);
        target.plugin().context("ViGEm Xbox 360 plugin")?;
        target.wait_ready().context("ViGEm Xbox 360 wait_ready")?;

        let notification = match target.request_notification() {
            Ok(req) => {
                let hub = hub.clone();
                let handle = req.spawn_thread(move |_target, data| {
                    let fb = PadFeedback::Rumble {
                        large: data.large_motor,
                        small: data.small_motor,
                    };
                    let report = build_usb_output_report(&fb);
                    hub.broadcast(report.to_vec());
                });
                tracing::info!("ViGEm Xbox 360 rumble notifications → DSVO feedback");
                Some(handle)
            }
            Err(e) => {
                tracing::warn!("Xbox rumble notifications unavailable: {e:?}");
                None
            }
        };

        tracing::info!("ViGEm Xbox 360 plugged (P2)");
        Ok(Self {
            target,
            _notification: notification,
        })
    }
}

impl PadBackend for VigemXbox {
    fn apply_ds_report(&mut self, report: &[u8; DS_USB_INPUT_LEN]) -> Result<()> {
        use anyhow::Context;
        use couchlink_pad::map_frame::stick_u8_to_i16;
        use couchlink_proto::pad_frame::buttons;

        // Map DualSense-shaped report bits onto XInput (same as windows_pad).
        let mut frame = couchlink_proto::PadFrame::neutral();
        frame.lx = report[1];
        frame.ly = report[2];
        frame.rx = report[3];
        frame.ry = report[4];
        frame.l2 = report[5];
        frame.r2 = report[6];
        let bl = report[8];
        let bh = report[9];
        let be = report[10];
        let dpad = bl & 0x0F;
        match dpad {
            0 => frame.buttons |= buttons::DPAD_UP,
            1 => frame.buttons |= buttons::DPAD_UP | buttons::DPAD_RIGHT,
            2 => frame.buttons |= buttons::DPAD_RIGHT,
            3 => frame.buttons |= buttons::DPAD_DOWN | buttons::DPAD_RIGHT,
            4 => frame.buttons |= buttons::DPAD_DOWN,
            5 => frame.buttons |= buttons::DPAD_DOWN | buttons::DPAD_LEFT,
            6 => frame.buttons |= buttons::DPAD_LEFT,
            7 => frame.buttons |= buttons::DPAD_UP | buttons::DPAD_LEFT,
            _ => {}
        }
        if bl & 0x10 != 0 {
            frame.buttons |= buttons::SQUARE;
        }
        if bl & 0x20 != 0 {
            frame.buttons |= buttons::CROSS;
        }
        if bl & 0x40 != 0 {
            frame.buttons |= buttons::CIRCLE;
        }
        if bl & 0x80 != 0 {
            frame.buttons |= buttons::TRIANGLE;
        }
        if bh & 0x01 != 0 {
            frame.buttons |= buttons::L1;
        }
        if bh & 0x02 != 0 {
            frame.buttons |= buttons::R1;
        }
        if bh & 0x04 != 0 {
            frame.buttons |= buttons::L2;
        }
        if bh & 0x08 != 0 {
            frame.buttons |= buttons::R2;
        }
        if bh & 0x10 != 0 {
            frame.buttons |= buttons::CREATE;
        }
        if bh & 0x20 != 0 {
            frame.buttons |= buttons::OPTIONS;
        }
        if bh & 0x40 != 0 {
            frame.buttons |= buttons::L3;
        }
        if bh & 0x80 != 0 {
            frame.buttons |= buttons::R3;
        }
        if be & 0x01 != 0 {
            frame.buttons |= buttons::PS;
        }

        let mut raw = 0u16;
        let b = frame.buttons;
        if b & buttons::CROSS != 0 {
            raw |= vigem_client::XButtons::A;
        }
        if b & buttons::CIRCLE != 0 {
            raw |= vigem_client::XButtons::B;
        }
        if b & buttons::SQUARE != 0 {
            raw |= vigem_client::XButtons::X;
        }
        if b & buttons::TRIANGLE != 0 {
            raw |= vigem_client::XButtons::Y;
        }
        if b & buttons::L1 != 0 {
            raw |= vigem_client::XButtons::LB;
        }
        if b & buttons::R1 != 0 {
            raw |= vigem_client::XButtons::RB;
        }
        if b & buttons::CREATE != 0 {
            raw |= vigem_client::XButtons::BACK;
        }
        if b & buttons::OPTIONS != 0 {
            raw |= vigem_client::XButtons::START;
        }
        if b & buttons::L3 != 0 {
            raw |= vigem_client::XButtons::LTHUMB;
        }
        if b & buttons::R3 != 0 {
            raw |= vigem_client::XButtons::RTHUMB;
        }
        if b & buttons::PS != 0 {
            raw |= vigem_client::XButtons::GUIDE;
        }
        if b & buttons::DPAD_UP != 0 {
            raw |= vigem_client::XButtons::UP;
        }
        if b & buttons::DPAD_DOWN != 0 {
            raw |= vigem_client::XButtons::DOWN;
        }
        if b & buttons::DPAD_LEFT != 0 {
            raw |= vigem_client::XButtons::LEFT;
        }
        if b & buttons::DPAD_RIGHT != 0 {
            raw |= vigem_client::XButtons::RIGHT;
        }

        let gamepad = vigem_client::XGamepad {
            buttons: vigem_client::XButtons::from(raw),
            left_trigger: frame.l2,
            right_trigger: frame.r2,
            thumb_lx: stick_u8_to_i16(frame.lx),
            thumb_ly: stick_u8_to_i16(255u8.saturating_sub(frame.ly)),
            thumb_rx: stick_u8_to_i16(frame.rx),
            thumb_ry: stick_u8_to_i16(255u8.saturating_sub(frame.ry)),
        };
        self.target
            .update(&gamepad)
            .context("ViGEm Xbox 360 update")?;
        Ok(())
    }
}
