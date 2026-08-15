//! Couchlink wire protocol shared by signaling, host, and client.
//!
//! Methodologies mirror Rohomieo: tagged JSON `type` discriminators in snake_case
//! for WebSocket signaling; media stays peer-to-peer. Pad state uses a compact
//! binary frame (`CLPD`) on the WebRTC DataChannel named `pad`.

pub mod pad_frame;
pub mod video_frame;
pub mod host_events;
pub mod signal;

pub use pad_frame::{PadFeedback, PadFrame, PAD_CHANNEL, PAD_MAGIC};
pub use video_frame::{
    annex_b_is_keyframe, VideoAccessUnit, VideoFragment, VIDEO_CHANNEL, VIDEO_MAGIC,
};
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
    fn answer_without_epoch_deserializes_as_zero() {
        let back = SignalMessage::from_json(r#"{"type":"answer","sdp":"v=0"}"#).unwrap();
        match back {
            SignalMessage::Answer { sdp, epoch, .. } => {
                assert_eq!(sdp, "v=0");
                assert_eq!(epoch, 0);
            }
            other => panic!("expected Answer, got {other:?}"),
        }
    }

    #[test]
    fn answer_with_epoch_roundtrips() {
        let msg = SignalMessage::Answer {
            sdp: "v=0".into(),
            epoch: 7,
            slot: 0,
        };
        let back = SignalMessage::from_json(&msg.to_json().unwrap()).unwrap();
        match back {
            SignalMessage::Answer { epoch: 7, .. } => {}
            other => panic!("expected Answer epoch 7, got {other:?}"),
        }
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
