use anyhow::{Context, Result};
use bytes::BytesMut;
use couchlink_proto::{PadFrame, SignalMessage, PAD_CHANNEL};
use std::sync::Arc;
use tokio::sync::mpsc;
use tracing::info;
use webrtc::api::interceptor_registry::register_default_interceptors;
use webrtc::api::media_engine::MediaEngine;
use webrtc::api::APIBuilder;
use webrtc::data_channel::RTCDataChannel;
use webrtc::ice_transport::ice_server::RTCIceServer;
use webrtc::interceptor::registry::Registry;
use webrtc::peer_connection::configuration::RTCConfiguration;
use webrtc::peer_connection::sdp::session_description::RTCSessionDescription;
use webrtc::peer_connection::RTCPeerConnection;

pub struct WebRtcPlayer {
    pub pc: Arc<RTCPeerConnection>,
    pub pad_dc: Arc<tokio::sync::Mutex<Option<Arc<RTCDataChannel>>>>,
}

impl WebRtcPlayer {
    pub async fn new(signal_out: mpsc::UnboundedSender<SignalMessage>) -> Result<Self> {
        let mut m = MediaEngine::default();
        m.register_default_codecs()?;
        let mut registry = Registry::new();
        registry = register_default_interceptors(registry, &mut m)?;
        let api = APIBuilder::new()
            .with_media_engine(m)
            .with_interceptor_registry(registry)
            .build();
        let config = RTCConfiguration {
            ice_servers: vec![RTCIceServer {
                urls: vec![],
                ..Default::default()
            }],
            ..Default::default()
        };
        let pc = Arc::new(api.new_peer_connection(config).await?);
        let pad_dc = Arc::new(tokio::sync::Mutex::new(None));

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

        let pad_slot = Arc::clone(&pad_dc);
        pc.on_data_channel(Box::new(move |dc| {
            let pad_slot = Arc::clone(&pad_slot);
            Box::pin(async move {
                if dc.label() == PAD_CHANNEL {
                    info!("pad channel attached");
                    *pad_slot.lock().await = Some(dc);
                }
            })
        }));

        pc.on_track(Box::new(move |track, _, _| {
            Box::pin(async move {
                info!("video track received: {}", track.codec().capability.mime_type);
                // Decode/display is left to a viewer frontend or SDL sink in a follow-up.
            })
        }));

        Ok(Self { pc, pad_dc })
    }

    pub async fn handle_offer(&self, sdp: String, signal_out: &mpsc::UnboundedSender<SignalMessage>) -> Result<()> {
        let offer = RTCSessionDescription::offer(sdp)?;
        self.pc.set_remote_description(offer).await?;
        let answer = self.pc.create_answer(None).await?;
        self.pc.set_local_description(answer).await?;
        let local = self.pc.local_description().await.context("local desc")?;
        signal_out.send(SignalMessage::Answer { sdp: local.sdp })?;
        Ok(())
    }

    pub async fn add_ice(&self, candidate: String, mid: Option<String>, mline: Option<u16>) -> Result<()> {
        use webrtc::ice_transport::ice_candidate::RTCIceCandidateInit;
        self.pc
            .add_ice_candidate(RTCIceCandidateInit {
                candidate,
                sdp_mid: mid,
                sdp_mline_index: mline,
                ..Default::default()
            })
            .await?;
        Ok(())
    }

    pub async fn send_pad(&self, frame: &PadFrame) -> Result<()> {
        let guard = self.pad_dc.lock().await;
        let Some(dc) = guard.as_ref() else {
            return Ok(());
        };
        let mut buf = BytesMut::new();
        frame.encode(&mut buf);
        dc.send(&bytes::Bytes::from(buf.to_vec())).await?;
        Ok(())
    }
}
