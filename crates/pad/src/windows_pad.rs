//! Windows virtual pad: DualSense VHID companion (preferred) + ViGEm DS4 / Xbox 360 fallbacks.

use anyhow::{bail, Context, Result};
use couchlink_proto::pad_frame::buttons;
use couchlink_proto::{PadFeedback, PadFrame};
use tracing::{info, warn};

use crate::map_frame::stick_u8_to_i16;
use crate::vhid_client::VhidClient;
use crate::virtual_pad::{VirtualPadBackend, VirtualPadConfig};

pub enum WindowsPad {
    /// Slot kept alongside the client so a mid-session reconnect (below)
    /// re-announces the same slot instead of silently landing on whatever
    /// the companion happens to assign next.
    DualSenseVhid(VhidClient, u8),
    VigemXbox360(VigemXbox),
    VigemDs4(VigemDs4),
    Noop,
}

impl WindowsPad {
    pub fn create(cfg: &VirtualPadConfig) -> Result<Self> {
        let slot = cfg.companion_slot;
        match cfg.backend {
            VirtualPadBackend::Noop => Ok(Self::Noop),
            VirtualPadBackend::DualSense => VhidClient::connect(slot)
                .map(|c| Self::DualSenseVhid(c, slot))
                .context("DualSense VHID companion not available"),
            VirtualPadBackend::Ds4 => VigemDs4::create().map(Self::VigemDs4),
            VirtualPadBackend::Xbox360 => VigemXbox::create().map(Self::VigemXbox360),
            VirtualPadBackend::Auto => {
                if let Ok(ds) = VhidClient::connect(slot) {
                    info!("Windows virtual pad: DualSense VHID companion");
                    return Ok(Self::DualSenseVhid(ds, slot));
                }
                warn!("DualSense VHID unavailable — trying ViGEm DS4");
                if let Ok(ds4) = VigemDs4::create() {
                    info!("Windows virtual pad: ViGEm DualShock 4");
                    return Ok(Self::VigemDs4(ds4));
                }
                warn!("ViGEm DS4 unavailable — trying ViGEm Xbox 360");
                if let Ok(x) = VigemXbox::create() {
                    info!("Windows virtual pad: ViGEm Xbox 360");
                    return Ok(Self::VigemXbox360(x));
                }
                bail!(
                    "no Windows virtual pad backend: install ViGEmBus \
                     (https://github.com/nefarius/ViGEmBus/releases) and/or run couchlink-ds-vhid"
                )
            }
        }
    }

    pub fn apply(&mut self, frame: &PadFrame) -> Result<()> {
        match self {
            // The companion is a separate process and is restarted whenever the
            // player's controller family changes, which breaks this socket
            // mid-session. Without reconnecting, every later frame returns
            // "Broken pipe" and the player's input is dead for good while video
            // keeps flowing — the failure looks like the pad, not the pipe.
            Self::DualSenseVhid(p, slot) => match p.apply(frame) {
                Ok(()) => Ok(()),
                Err(e) => {
                    let this_slot = *slot;
                    let Ok(mut fresh) = VhidClient::connect(this_slot) else {
                        return Err(e);
                    };
                    tracing::info!("DualSense VHID companion reconnected after {e}");
                    let r = fresh.apply(frame);
                    *self = Self::DualSenseVhid(fresh, this_slot);
                    r
                }
            },
            Self::VigemXbox360(p) => p.apply(frame),
            Self::VigemDs4(p) => p.apply(frame),
            Self::Noop => Ok(()),
        }
    }

    pub fn poll_feedback(&mut self) -> Result<Vec<PadFeedback>> {
        match self {
            Self::DualSenseVhid(p, _) => p.poll_feedback(),
            _ => Ok(Vec::new()),
        }
    }
}

pub struct VigemXbox {
    target: vigem_client::Xbox360Wired<vigem_client::Client>,
}

impl VigemXbox {
    pub fn create() -> Result<Self> {
        let client =
            vigem_client::Client::connect().context("ViGEmBus connect (is the driver installed?)")?;
        let id = vigem_client::TargetId::XBOX360_WIRED;
        let mut target = vigem_client::Xbox360Wired::new(client, id);
        target.plugin().context("ViGEm Xbox 360 plugin")?;
        target.wait_ready().context("ViGEm Xbox 360 wait_ready")?;
        Ok(Self { target })
    }

    pub fn apply(&mut self, frame: &PadFrame) -> Result<()> {
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

pub struct VigemDs4 {
    target: vigem_client::DualShock4Wired<vigem_client::Client>,
}

impl VigemDs4 {
    pub fn create() -> Result<Self> {
        let client = vigem_client::Client::connect().context("ViGEmBus connect")?;
        let id = vigem_client::TargetId::DUALSHOCK4_WIRED;
        let mut target = vigem_client::DualShock4Wired::new(client, id);
        target.plugin().context("ViGEm DS4 plugin")?;
        target.wait_ready().context("ViGEm DS4 wait_ready")?;
        Ok(Self { target })
    }

    pub fn apply(&mut self, frame: &PadFrame) -> Result<()> {
        let mut btn: u16 = ds4_dpad_hat(frame.buttons);
        let b = frame.buttons;
        if b & buttons::SQUARE != 0 {
            btn |= 1 << 4;
        }
        if b & buttons::CROSS != 0 {
            btn |= 1 << 5;
        }
        if b & buttons::CIRCLE != 0 {
            btn |= 1 << 6;
        }
        if b & buttons::TRIANGLE != 0 {
            btn |= 1 << 7;
        }
        if b & buttons::L1 != 0 {
            btn |= 1 << 8;
        }
        if b & buttons::R1 != 0 {
            btn |= 1 << 9;
        }
        if b & buttons::L2 != 0 {
            btn |= 1 << 10;
        }
        if b & buttons::R2 != 0 {
            btn |= 1 << 11;
        }
        if b & buttons::CREATE != 0 {
            btn |= 1 << 12;
        }
        if b & buttons::OPTIONS != 0 {
            btn |= 1 << 13;
        }
        if b & buttons::L3 != 0 {
            btn |= 1 << 14;
        }
        if b & buttons::R3 != 0 {
            btn |= 1 << 15;
        }
        let mut special = 0u8;
        if b & buttons::PS != 0 {
            special |= 1;
        }
        if b & buttons::TOUCH != 0 {
            special |= 2;
        }

        let report = vigem_client::DS4Report {
            thumb_lx: frame.lx,
            thumb_ly: frame.ly,
            thumb_rx: frame.rx,
            thumb_ry: frame.ry,
            buttons: btn,
            special,
            trigger_l: frame.l2,
            trigger_r: frame.r2,
        };
        self.target.update(&report).context("ViGEm DS4 update")?;
        Ok(())
    }
}

fn ds4_dpad_hat(b: u32) -> u16 {
    let u = b & buttons::DPAD_UP != 0;
    let d = b & buttons::DPAD_DOWN != 0;
    let l = b & buttons::DPAD_LEFT != 0;
    let r = b & buttons::DPAD_RIGHT != 0;
    (match (u, d, l, r) {
        (true, false, false, false) => 0,
        (true, false, false, true) => 1,
        (false, false, false, true) => 2,
        (false, true, false, true) => 3,
        (false, true, false, false) => 4,
        (false, true, true, false) => 5,
        (false, false, true, false) => 6,
        (true, false, true, false) => 7,
        _ => 8,
    }) as u16
}
