use anyhow::{Context, Result};
use bytes::BytesMut;
use couchlink_proto::{PadFrame, SignalMessage, PAD_CHANNEL};
use std::sync::Arc;
use tokio::sync::mpsc;
use tracing::{info, warn};

use crate::decode::{DecodedFrame, H264Decoder};
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
    pub async fn new(
        signal_out: mpsc::UnboundedSender<SignalMessage>,
        turn_url: Option<String>,
        turn_user: Option<String>,
        turn_pass: Option<String>,
    ) -> Result<(Self, mpsc::UnboundedReceiver<DecodedFrame>)> {
        let mut m = MediaEngine::default();
        m.register_default_codecs()?;
        let mut registry = Registry::new();
        registry = register_default_interceptors(registry, &mut m)?;
        let api = APIBuilder::new()
            .with_media_engine(m)
            .with_interceptor_registry(registry)
            .build();
        // Public STUN for NAT discovery, plus our own TURN relay (scripts/start-turn.sh)
        // for symmetric-NAT/CGNAT peers STUN alone can't punch through.
        let mut ice_servers = vec![RTCIceServer {
            urls: vec![
                "stun:stun.l.google.com:19302".to_owned(),
                "stun:stun1.l.google.com:19302".to_owned(),
            ],
            ..Default::default()
        }];
        if let (Some(url), Some(user), Some(pass)) = (turn_url, turn_user, turn_pass) {
            ice_servers.push(RTCIceServer {
                urls: vec![url],
                username: user,
                credential: pass,
                ..Default::default()
            });
        }
        let config = RTCConfiguration {
            ice_servers,
            ..Default::default()
        };
        let pc = Arc::new(api.new_peer_connection(config).await?);
        let pad_dc = Arc::new(tokio::sync::Mutex::new(None));

        let signal_ice = signal_out.clone();
        pc.on_ice_candidate(Box::new(move |c| {
            let signal_ice = signal_ice.clone();
            Box::pin(async move {
                if let Some(c) = c {
                    if let Ok(init) = c.to_json() {
                        let _ = signal_ice.send(SignalMessage::IceCandidate {
                            candidate: init.candidate,
                            sdp_mid: init.sdp_mid,
                            sdp_mline_index: init.sdp_mline_index,
                        });
                    }
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

        let (nal_tx, nal_rx) = mpsc::unbounded_channel::<bytes::Bytes>();
        let (frame_tx, frame_rx) = mpsc::unbounded_channel::<DecodedFrame>();

        std::thread::Builder::new()
            .name("couchlink-decode".into())
            .spawn(move || {
                let mut decoder = match H264Decoder::new() {
                    Ok(d) => d,
                    Err(e) => {
                        tracing::error!("failed to init h264 decoder: {e}");
                        return;
                    }
                };
                let mut nal_rx = nal_rx;
                while let Some(nal) = nal_rx.blocking_recv() {
                    match decoder.decode(&nal) {
                        Ok(Some(frame)) => {
                            if frame_tx.send(frame).is_err() {
                                break; // viewer gone
                            }
                        }
                        Ok(None) => {}
                        Err(e) => tracing::warn!("decode error: {e}"),
                    }
                }
            })
            .expect("spawn decode thread");

        pc.on_track(Box::new(move |track, _, _| {
            let nal_tx = nal_tx.clone();
            Box::pin(async move {
                info!("video track received: {}", track.codec().capability.mime_type);
                let mut depacketizer = rtp::codecs::h264::H264Packet::default();
                depacketizer.is_avc = false; // false => depacketize() emits Annex-B (start-code) NALs
                loop {
                    match track.read_rtp().await {
                        Ok((packet, _attrs)) => {
                            use rtp::packetizer::Depacketizer;
                            match depacketizer.depacketize(&packet.payload) {
                                Ok(nal) if !nal.is_empty() => {
                                    if nal_tx.send(nal).is_err() {
                                        break; // decode thread gone, stop reading
                                    }
                                }
                                Ok(_) => {} // mid-fragment, nothing to emit yet
                                Err(e) => warn!("rtp depacketize error: {e}"),
                            }
                        }
                        Err(e) => {
                            warn!("video track read_rtp ended: {e}");
                            break;
                        }
                    }
                }
            })
        }));

        Ok((Self { pc, pad_dc }, frame_rx))
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
