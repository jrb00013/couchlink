//! Apply PadFeedback to the local DualSense (rumble / lightbar) when host sends it.

use anyhow::Result;
use couchlink_proto::PadFeedback;
use tracing::debug;

pub fn apply_feedback(fb: &PadFeedback) -> Result<()> {
    match fb {
        PadFeedback::Rumble { large, small } => {
            debug!("rumble large={large} small={small}");
        }
        PadFeedback::Lightbar { r, g, b } => {
            debug!("lightbar {r},{g},{b}");
        }
        PadFeedback::PlayerLed { mask } => {
            debug!("player led mask={mask}");
        }
    }
    Ok(())
}
