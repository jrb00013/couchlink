//! Latency-critical WebRTC helpers — kept pure so regression tests can lock them.

/// URI negotiated in SDP / stamped on every outbound H.264 packet.
pub const PLAYOUT_DELAY_URI: &str =
    "http://www.webrtc.org/experiments/rtp-hdrext/playout-delay";

/// Gaming target: play as soon as a full frame is assembled (10ms units → 0ms).
pub fn gaming_playout_delay() -> (u16, u16) {
    (0, 0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use bytes::BytesMut;
    use rtp::extension::playout_delay_extension::PlayoutDelayExtension;
    use webrtc::api::media_engine::MediaEngine;
    use webrtc::api::APIBuilder;
    use webrtc::peer_connection::configuration::RTCConfiguration;
    use webrtc::rtp_transceiver::rtp_codec::{
        RTCRtpCodecCapability, RTCRtpHeaderExtensionCapability, RTPCodecType,
    };
    use webrtc::track::track_local::track_local_static_sample::TrackLocalStaticSample;
    use webrtc::track::track_local::TrackLocal;
    use webrtc::util::marshal::{Marshal, MarshalSize, Unmarshal};
    use webrtc::api::media_engine::MIME_TYPE_H264;
    use std::sync::Arc;

    #[test]
    fn gaming_playout_delay_is_zero() {
        assert_eq!(gaming_playout_delay(), (0, 0));
    }

    #[test]
    fn playout_delay_zero_roundtrips() {
        let ext = PlayoutDelayExtension::new(0, 0);
        let mut raw = BytesMut::new();
        raw.resize(ext.marshal_size(), 0);
        ext.marshal_to(&mut raw).expect("marshal");
        let frozen = raw.freeze();
        let out = PlayoutDelayExtension::unmarshal(&mut frozen.clone()).expect("unmarshal");
        assert_eq!(out.min_delay, 0);
        assert_eq!(out.max_delay, 0);
    }

    #[tokio::test]
    async fn offer_sdp_advertises_playout_delay() {
        let mut m = MediaEngine::default();
        m.register_default_codecs().expect("codecs");
        m.register_header_extension(
            RTCRtpHeaderExtensionCapability {
                uri: PLAYOUT_DELAY_URI.into(),
            },
            RTPCodecType::Video,
            None,
        )
        .expect("register extension");

        let api = APIBuilder::new().with_media_engine(m).build();
        let pc = api
            .new_peer_connection(RTCConfiguration::default())
            .await
            .expect("pc");
        let video = Arc::new(TrackLocalStaticSample::new(
            RTCRtpCodecCapability {
                mime_type: MIME_TYPE_H264.to_owned(),
                ..Default::default()
            },
            "video".into(),
            "couchlink".into(),
        ));
        pc.add_track(Arc::clone(&video) as Arc<dyn TrackLocal + Send + Sync>)
            .await
            .expect("add track");
        let offer = pc.create_offer(None).await.expect("offer");
        assert!(
            offer.sdp.contains("playout-delay"),
            "SDP missing playout-delay extmap:\n{}",
            offer.sdp
        );
        let _ = pc.close().await;
    }
}
