//! Optional host→player control messages (JSON) on the pad channel.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum HostToPlayer {
    SessionReady { preset: String },
    EmulatorHint { name: String },
    LatencyProbe { t_ms: u64 },
}
