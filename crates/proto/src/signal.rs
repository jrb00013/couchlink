//! WebSocket signaling envelope — Rohomieo methodology adapted for co-play.
//! Host registers with session_id + PIN; friend registers as player.
//! Offer/answer/ICE relay only; video+pad never transit the signaling server.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SignalMessage {
    RegisterHost {
        session_id: String,
        pin: String,
        device_name: Option<String>,
        /// e.g. "1080p60", "720p60", "720p30"
        preset: Option<String>,
        emulator: Option<String>,
    },
    RegisterPlayer {
        session_id: String,
        pin: String,
        player_name: Option<String>,
    },
    Registered {
        role: Role,
        session_id: String,
    },
    Error {
        message: String,
    },
    Offer {
        sdp: String,
        /// Monotonic per host session; players ignore stale offers while connected.
        #[serde(default)]
        epoch: u64,
    },
    Answer {
        sdp: String,
        /// Echo of the offer epoch so the host can drop answers for superseded offers
        /// (rapid rejoin / double-tab races). Older clients omit this (treated as 0).
        #[serde(default)]
        epoch: u64,
    },
    IceCandidate {
        candidate: String,
        #[serde(rename = "sdpMid")]
        sdp_mid: Option<String>,
        #[serde(rename = "sdpMLineIndex")]
        sdp_mline_index: Option<u16>,
    },
    Heartbeat,
    Pong,
    /// Player asks host to send a new SDP offer (e.g. after WebRTC failed) without re-registering.
    RequestOffer,
    PeerJoined {
        role: Role,
        /// Incremented when a new player WebSocket replaces an empty slot.
        #[serde(default)]
        epoch: u64,
    },
    PeerLeft,
    /// Host announces stream ready (codec / resolution).
    StreamInfo {
        width: u32,
        height: u32,
        fps: u32,
        codec: String,
        /// False when the host sees a black/empty capture (common: WSL host + Windows game).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        capture_ok: Option<bool>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        capture_hint: Option<String>,
    },
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    Host,
    Player,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StreamPreset {
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    pub bitrate_kbps: u32,
}

impl StreamPreset {
    pub const P1080_60: Self = Self {
        width: 1920,
        height: 1080,
        fps: 60,
        // Screen/UI text needs headroom — 12 Mbps at 1080p60 looked crunchy on LAN.
        bitrate_kbps: 18_000,
    };
    pub const P1080_30: Self = Self {
        width: 1920,
        height: 1080,
        fps: 30,
        bitrate_kbps: 10_000,
    };
    pub const P720_60: Self = Self {
        width: 1280,
        height: 720,
        fps: 60,
        bitrate_kbps: 10_000,
    };
    pub const P720_30: Self = Self {
        width: 1280,
        height: 720,
        fps: 30,
        bitrate_kbps: 5_000,
    };

    pub fn parse(name: &str) -> Option<Self> {
        match name.trim().to_ascii_lowercase().as_str() {
            "1080p60" | "hd60" => Some(Self::P1080_60),
            "1080p30" | "hd30" => Some(Self::P1080_30),
            "720p60" => Some(Self::P720_60),
            "720p30" | "default" => Some(Self::P720_30),
            _ => None,
        }
    }
}

impl SignalMessage {
    pub fn to_json(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string(self)
    }

    pub fn from_json(s: &str) -> Result<Self, serde_json::Error> {
        serde_json::from_str(s)
    }
}
