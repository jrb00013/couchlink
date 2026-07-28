//! Couchlink wire protocol shared by signaling, host, and client.
//!
//! Methodologies mirror Rohomieo: tagged JSON `type` discriminators in snake_case
//! for WebSocket signaling; media stays peer-to-peer. Pad state uses a compact
//! binary frame (`CLPD`) on the WebRTC DataChannel named `pad`.

pub mod pad_frame;
pub mod signal;

pub use pad_frame::{PadFeedback, PadFrame, PAD_CHANNEL, PAD_MAGIC};
pub use signal::{Role, SignalMessage, StreamPreset};

#[cfg(test)]
mod tests {
    use super::*;
    use bytes::BytesMut;

    #[test]
    fn signal_register_host_roundtrip() {
        let msg = SignalMessage::RegisterHost {
            session_id: "abc".into(),
            pin: "123456".into(),
            device_name: Some("desk".into()),
            preset: Some("1080p60".into()),
            emulator: Some("rpcs3".into()),
        };
        let json = msg.to_json().unwrap();
        let back = SignalMessage::from_json(&json).unwrap();
        assert!(matches!(back, SignalMessage::RegisterHost { .. }));
    }

    #[test]
    fn pad_frame_roundtrip() {
        let mut f = PadFrame::neutral();
        f.seq = 42;
        f.buttons = pad_frame::buttons::CROSS | pad_frame::buttons::R1;
        f.l2 = 200;
        let mut buf = BytesMut::new();
        f.encode(&mut buf);
        let back = PadFrame::decode(&buf).unwrap();
        assert_eq!(back, f);
    }

    #[test]
    fn preset_parse() {
        assert_eq!(StreamPreset::parse("1080p60").unwrap().fps, 60);
        assert_eq!(StreamPreset::parse("720p30").unwrap().width, 1280);
    }
}
