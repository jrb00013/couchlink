//! WebRTC host peer — video track + `pad` DataChannel (Rohomieo offer flow).

use anyhow::{Context, Result};
use bytes::BytesMut;
use couchlink_pad::{VirtualPad, VirtualPadConfig};
use couchlink_proto::{PadFeedback, PadFrame, SignalMessage, PAD_CHANNEL};
use std::sync::Arc;
use tokio::sync::{mpsc, Mutex};
use tracing::{info, warn};
use webrtc::api::interceptor_registry::register_default_interceptors;
use webrtc::api::media_engine::{MediaEngine, MIME_TYPE_H264};
use webrtc::api::APIBuilder;
use webrtc::data_channel::data_channel_message::DataChannelMessage;
use webrtc::data_channel::RTCDataChannel;
use webrtc::ice_transport::ice_server::RTCIceServer;
use webrtc::interceptor::registry::Registry;
use webrtc::peer_connection::configuration::RTCConfiguration;
use webrtc::peer_connection::sdp::session_description::RTCSessionDescription;
use webrtc::peer_connection::RTCPeerConnection;
use webrtc::rtp_transceiver::rtp_codec::RTCRtpCodecCapability;
use webrtc::track::track_local::track_local_static_sample::TrackLocalStaticSample;
use webrtc::track::track_local::TrackLocal;
use webrtc::media::Sample;
use std::time::Duration;

pub struct WebRtcHost {
    pub pc: Arc<RTCPeerConnection>,
    pub video: Arc<TrackLocalStaticSample>,
    pub pad_tx: mpsc::UnboundedSender<PadFrame>,
}

impl WebRtcHost {
    pub async fn new(
        signal_out: mpsc::UnboundedSender<SignalMessage>,
        pad_device: Arc<Mutex<VirtualPad>>,
        as_bluetooth: bool,
    ) -> Result<(Self, mpsc::UnboundedReceiver<PadFrame>)> {
        let _ = as_bluetooth;
        let mut m = MediaEngine::default();
        m.register_default_codecs()?;
        let mut registry = Registry::new();
        registry = register_default_interceptors(registry, &mut m)?;
        let api = APIBuilder::new()
            .with_media_engine(m)
            .with_interceptor_registry(registry)
            .build();

        // Empty ICE servers → LAN / WireGuard only (Rohomieo security posture).
        let config = RTCConfiguration {
            ice_servers: vec![RTCIceServer {
                urls: vec![],
                ..Default::default()
            }],
            ..Default::default()
        };
        let pc = Arc::new(api.new_peer_connection(config).await?);

        let video = Arc::new(TrackLocalStaticSample::new(
            RTCRtpCodecCapability {
                mime_type: MIME_TYPE_H264.to_owned(),
                ..Default::default()
            },
            "video".to_owned(),
            "couchlink".to_owned(),
        ));
        pc.add_track(Arc::clone(&video) as Arc<dyn TrackLocal + Send + Sync>)
            .await?;

        let (pad_tx, pad_rx) = mpsc::unbounded_channel::<PadFrame>();
        let pad_tx_dc = pad_tx.clone();
        let pad_device_dc = Arc::clone(&pad_device);

        let pc2 = Arc::clone(&pc);
        let signal_ice = signal_out.clone();
        pc.on_ice_candidate(Box::new(move |c| {
            let signal_ice = signal_ice.clone();
            Box::pin(async move {
                if let Some(c) = c {
                    let _ = signal_ice.send(SignalMessage::IceCandidate {
                        candidate: c.to_json().await.unwrap_or_default().candidate,
                        sdp_mid: c.sdp_mid.clone(),
                        sdp_mline_index: c.sdp_mline_index.map(|v| v as u16),
                    });
                }
            })
        }));

        // Create pad data channel (host→negotiated with offer)
        let dc = pc2.create_data_channel(PAD_CHANNEL, None).await?;
        setup_pad_channel(dc, pad_tx_dc, pad_device_dc).await;

        Ok((
            Self {
                pc,
                video,
                pad_tx,
            },
            pad_rx,
        ))
    }

    pub async fn create_and_send_offer(
        &self,
        signal_out: &mpsc::UnboundedSender<SignalMessage>,
    ) -> Result<()> {
        let offer = self.pc.create_offer(None).await?;
        self.pc.set_local_description(offer).await?;
        let local = self
            .pc
            .local_description()
            .await
            .context("local description")?;
        signal_out.send(SignalMessage::Offer { sdp: local.sdp })?;
        Ok(())
    }

    pub async fn handle_answer(&self, sdp: String) -> Result<()> {
        let answer = RTCSessionDescription::answer(sdp)?;
        self.pc.set_remote_description(answer).await?;
        Ok(())
    }

    pub async fn add_ice(&self, candidate: String, mid: Option<String>, mline: Option<u16>) -> Result<()> {
        use webrtc::ice_transport::ice_candidate::RTCIceCandidateInit;
        let init = RTCIceCandidateInit {
            candidate,
            sdp_mid: mid,
            sdp_mline_index: mline,
            ..Default::default()
        };
        self.pc.add_ice_candidate(init).await?;
        Ok(())
    }

    pub async fn push_h264(&self, annex_b: Vec<u8>, duration: Duration) -> Result<()> {
        self.video
            .write_sample(&Sample {
                data: bytes::Bytes::from(annex_b),
                duration,
                ..Default::default()
            })
            .await?;
        Ok(())
    }
}

async fn setup_pad_channel(
    dc: Arc<RTCDataChannel>,
    pad_tx: mpsc::UnboundedSender<PadFrame>,
    pad_device: Arc<Mutex<VirtualPad>>,
) {
    dc.on_open(Box::new(move || {
        info!("pad datachannel open");
        Box::pin(async {})
    }));

    dc.on_message(Box::new(move |msg: DataChannelMessage| {
        let pad_tx = pad_tx.clone();
        let pad_device = Arc::clone(&pad_device);
        Box::pin(async move {
            if msg.is_string {
                // feedback JSON ignored on host inbound (player→host is binary pads)
                if let Ok(text) = std::str::from_utf8(&msg.data) {
                    if let Ok(_fb) = serde_json::from_str::<PadFeedback>(text) {
                        // player shouldn't send feedback; ignore
                    }
                }
                return;
            }
            match PadFrame::decode(&msg.data) {
                Ok(frame) => {
                    let _ = pad_tx.send(frame);
                    let mut guard = pad_device.lock().await;
                    if let Err(e) = guard.apply(&frame) {
                        warn!("virtual pad apply: {e}");
                    }
                }
                Err(e) => warn!("bad pad frame: {e}"),
            }
        })
    }));
}

pub fn create_virtual_pad(as_bluetooth: bool) -> Result<VirtualPad> {
    let mut cfg = VirtualPadConfig::default();
    cfg.as_bluetooth = as_bluetooth;
    VirtualPad::create(cfg)
}

/// Helper kept for tests / demos without WebRTC.
pub fn apply_pad_bytes(pad: &mut VirtualPad, data: &[u8]) -> Result<()> {
    let frame = PadFrame::decode(data)?;
    pad.apply(&frame)?;
    let _ = BytesMut::new();
    Ok(())
}
