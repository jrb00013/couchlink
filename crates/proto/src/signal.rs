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
        /// Player slot assigned by the session (1..=3); always 0 for the host role.
        #[serde(default)]
        slot: u8,
    },
    Error {
        message: String,
    },
    Offer {
        sdp: String,
        /// Monotonic per host session; players ignore stale offers while connected.
        #[serde(default)]
        epoch: u64,
        /// Player slot this offer is for — the host stamps its current player so the
        /// signaling server can route instead of blind-relaying to one socket.
        #[serde(default)]
        slot: u8,
    },
    Answer {
        sdp: String,
        /// Echo of the offer epoch so the host can drop answers for superseded offers
        /// (rapid rejoin / double-tab races). Older clients omit this (treated as 0).
        #[serde(default)]
        epoch: u64,
        /// Player slot this answer came from. The signaling server stamps it from the
        /// connection's registered slot, never trusting a client-supplied value.
        #[serde(default)]
        slot: u8,
    },
    IceCandidate {
        candidate: String,
        #[serde(rename = "sdpMid")]
        sdp_mid: Option<String>,
        #[serde(rename = "sdpMLineIndex")]
        sdp_mline_index: Option<u16>,
        /// Same routing role as `Offer.slot`: stamped by the host on the way out,
        /// stamped by the signaling server on the way in.
        #[serde(default)]
        slot: u8,
    },
    Heartbeat,
    Pong,
    /// Player asks host to send a new SDP offer (e.g. after WebRTC failed) without re-registering.
    RequestOffer {
        /// Player slot making the request (stamped by the signaling server).
        #[serde(default)]
        slot: u8,
    },
    PeerJoined {
        role: Role,
        /// Incremented when a new player WebSocket replaces an empty slot.
        #[serde(default)]
        epoch: u64,
        /// Which of the session's 3 player slots this player now holds (1-based).
        #[serde(default)]
        slot: u8,
    },
    PeerLeft {
        /// Player slot that left (stamped by the signaling server so the host
        /// can drop exactly that peer connection). 0 from a legacy server that
        /// predates slots, or on host leave broadcasts to players.
        #[serde(default)]
        slot: u8,
    },
    /// Session occupancy snapshot, broadcast to the host and every player whenever
    /// a player joins or leaves so clients can show "N/3 players connected".
    PlayersStatus {
        occupied: u8,
        max: u8,
    },
    /// Player reports which controller family it is actually holding.
    ///
    /// The browser Gamepad API normalises every pad to the same layout, so an
    /// Xbox pad and a DualSense arrive byte-identical in `PadFrame` — the host
    /// cannot tell them apart from input alone. Without this the host guesses,
    /// and a guess that misses means the emulator binds a device the player
    /// does not have and silently drops every button.
    PadInfo {
        /// `xbox`, `dualsense`, or `generic` (see web `controllerKind`).
        kind: String,
        /// Raw `Gamepad.id`, for logs when the classification looks wrong.
        #[serde(default)]
        id: String,
        /// Player slot reporting its pad (stamped by the signaling server).
        #[serde(default)]
        slot: u8,
    },
    /// Broadcast echo of a player's `PadInfo`, sent to the host *and every
    /// player* (not just relayed to the host) so a controller debug view can
    /// show every seated player's controller, not only your own — the browser
    /// otherwise has no way to see what anyone else in the session is holding.
    PlayerPadInfo {
        slot: u8,
        kind: String,
        #[serde(default)]
        id: String,
    },
    /// Player reports which video path it is actually presenting from.
    ///
    /// The host otherwise writes every frame to both the RTP track and the
    /// CLVD DataChannel, because it has no way to know which one the browser
    /// paints — double the per-frame send work, and two streams competing
    /// inside one congestion controller. `path` is `"webcodecs"` or `"rtp"`;
    /// an unrecognised value is treated as unknown (send both).
    PresentPath {
        path: String,
        /// Player slot reporting its path (stamped by the signaling server).
        #[serde(default)]
        slot: u8,
    },
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
    /// Host pipeline telemetry — where each frame's time goes on the host side.
    ///
    /// Sent every stats window (~5s) so the debug panel can name the slow hop
    /// from the host's half of the path too: the browser already reports its
    /// own decode/paint numbers, and these are the host's capture/scale/encode/
    /// push averages for the same window.
    HostStats {
        /// Frames per second the host pushed to the wire in the last window.
        fps: f64,
        /// Frames pushed (sent) in the last window.
        frames_out: u64,
        /// Frames dropped or shed in the last window.
        dropped_frames: u64,
        /// Drop share in the last window (0-100).
        drop_pct: u32,
        /// Per-frame stage averages in the last window, milliseconds.
        capture_ms: f64,
        scale_ms: f64,
        encode_ms: f64,
        push_ms: f64,
        /// Stage dominating host per-frame time in the last window.
        dominant_stage: String,
        /// What the encoder is currently commanded to produce.
        target_width: u32,
        target_height: u32,
        target_fps: u32,
        target_bitrate_kbps: u32,
        #[serde(default)]
        age_p50_ms: f64,
        #[serde(default)]
        age_p95_ms: f64,
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
        // 10 Mbps × 3 friends blew the push budget even with IDR-only RTP.
        // 5 Mbps holds 60 fps on 3-friend WAN; governor climbs back if clean.
        bitrate_kbps: 5_000,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn present_path_round_trips() {
        let m = SignalMessage::PresentPath {
            path: "webcodecs".into(),
            slot: 2,
        };
        let s = m.to_json().unwrap();
        assert!(s.contains("\"type\":\"present_path\""));
        assert!(s.contains("\"path\":\"webcodecs\""));
        assert!(s.contains("\"slot\":2"));
        let back = SignalMessage::from_json(&s).unwrap();
        match back {
            SignalMessage::PresentPath { path, slot } => {
                assert_eq!(path, "webcodecs");
                assert_eq!(slot, 2);
            }
            other => panic!("wrong variant: {other:?}"),
        }
    }

    #[test]
    fn missing_slot_fields_default_to_zero() {
        // Older clients/hosts that predate slots still parse.
        for raw in [
            r#"{"type":"offer","sdp":"v=0"}"#,
            r#"{"type":"answer","sdp":"v=0"}"#,
            r#"{"type":"ice_candidate","candidate":"candidate:1 1 udp 2 3.4.5.6 7 typ host"}"#,
            r#"{"type":"pad_info","kind":"xbox","id":"Xbox Controller"}"#,
            r#"{"type":"present_path","path":"webcodecs"}"#,
            r#"{"type":"request_offer"}"#,
            r#"{"type":"peer_joined","role":"player","epoch":1}"#,
            r#"{"type":"registered","role":"player","session_id":"abc"}"#,
            r#"{"type":"peer_left"}"#,
        ] {
            match SignalMessage::from_json(raw) {
                Ok(m) => match m {
                    SignalMessage::Offer { slot: 0, .. }
                    | SignalMessage::Answer { slot: 0, .. }
                    | SignalMessage::IceCandidate { slot: 0, .. }
                    | SignalMessage::PadInfo { slot: 0, .. }
                    | SignalMessage::PresentPath { slot: 0, .. }
                    | SignalMessage::RequestOffer { slot: 0 }
                    | SignalMessage::PeerJoined { slot: 0, .. }
                    | SignalMessage::PeerLeft { slot: 0 }
                    | SignalMessage::Registered { slot: 0, .. } => {}
                    other => panic!("expected a defaulted slot, got {other:?}"),
                },
                Err(e) => panic!("failed to parse legacy {raw}: {e}"),
            }
        }
    }

    #[test]
    fn request_offer_with_slot_round_trips() {
        let m = SignalMessage::RequestOffer { slot: 3 };
        let back = SignalMessage::from_json(&m.to_json().unwrap()).unwrap();
        match back {
            SignalMessage::RequestOffer { slot } => assert_eq!(slot, 3),
            other => panic!("wrong variant: {other:?}"),
        }
    }

    #[test]
    fn players_status_round_trips() {
        let m = SignalMessage::PlayersStatus {
            occupied: 3,
            max: 4,
        };
        let s = m.to_json().unwrap();
        assert!(s.contains("\"type\":\"players_status\""));
        assert!(s.contains("\"occupied\":3"));
        assert!(s.contains("\"max\":4"));
        let back = SignalMessage::from_json(&s).unwrap();
        match back {
            SignalMessage::PlayersStatus { occupied, max } => {
                assert_eq!(occupied, 3);
                assert_eq!(max, 4);
            }
            other => panic!("wrong variant: {other:?}"),
        }
    }

    #[test]
    fn offer_and_answer_slots_round_trip() {
        let m = SignalMessage::Offer {
            sdp: "v=0".into(),
            epoch: 5,
            slot: 1,
        };
        let back = SignalMessage::from_json(&m.to_json().unwrap()).unwrap();
        match back {
            SignalMessage::Offer { slot: 1, epoch: 5, .. } => {}
            other => panic!("wrong variant: {other:?}"),
        }
    }

    #[test]
    fn peer_left_round_trips_with_slot() {
        let m = SignalMessage::PeerLeft { slot: 2 };
        let s = m.to_json().unwrap();
        assert!(s.contains("\"type\":\"peer_left\""));
        assert!(s.contains("\"slot\":2"));
        let back = SignalMessage::from_json(&s).unwrap();
        match back {
            SignalMessage::PeerLeft { slot } => assert_eq!(slot, 2),
            other => panic!("wrong variant: {other:?}"),
        }
    }

    #[test]
    fn player_pad_info_round_trips() {
        let m = SignalMessage::PlayerPadInfo {
            slot: 2,
            kind: "dualsense".into(),
            id: "DualSense Wireless Controller".into(),
        };
        let s = m.to_json().unwrap();
        assert!(s.contains("\"type\":\"player_pad_info\""));
        let back = SignalMessage::from_json(&s).unwrap();
        match back {
            SignalMessage::PlayerPadInfo { slot, kind, id } => {
                assert_eq!(slot, 2);
                assert_eq!(kind, "dualsense");
                assert_eq!(id, "DualSense Wireless Controller");
            }
            other => panic!("wrong variant: {other:?}"),
        }
    }
}
