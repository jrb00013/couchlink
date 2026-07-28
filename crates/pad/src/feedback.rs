//! Map host rumble/lightbar feedback toward the player's real DualSense (via client).

use couchlink_proto::PadFeedback;

pub fn encode_feedback_json(fb: &PadFeedback) -> Result<String, serde_json::Error> {
    serde_json::to_string(fb)
}
