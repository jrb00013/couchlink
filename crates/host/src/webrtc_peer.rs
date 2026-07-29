//! WebRTC host peer — video track + `pad` / `video` DataChannels (Rohomieo offer flow).

use anyhow::{Context, Result};
use bytes::{BytesMut, Bytes};
use couchlink_pad::{VirtualPad, VirtualPadConfig};
use couchlink_proto::{
    PadFeedback, PadFrame, SignalMessage, VideoAccessUnit, PAD_CHANNEL, VIDEO_CHANNEL,
};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::Arc;
use tokio::sync::{mpsc, Mutex};
use tracing::{info, warn};
use webrtc::api::interceptor_registry::register_default_interceptors;
use webrtc::api::media_engine::{MediaEngine, MIME_TYPE_H264};
use webrtc::api::setting_engine::SettingEngine;
use webrtc::api::APIBuilder;
use webrtc::ice_transport::ice_candidate_type::RTCIceCandidateType;
use webrtc::data_channel::data_channel_message::DataChannelMessage;
use webrtc::data_channel::RTCDataChannel;
use webrtc::ice_transport::ice_server::RTCIceServer;
use webrtc::interceptor::registry::Registry;
use webrtc::peer_connection::configuration::RTCConfiguration;
use webrtc::peer_connection::sdp::session_description::RTCSessionDescription;
use webrtc::peer_connection::RTCPeerConnection;
use webrtc::rtp_transceiver::rtp_codec::RTCRtpCodecCapability;
use webrtc::track::track_local::track_local_static_sample::TrackLocalStaticSample;
use webrtc::rtcp::payload_feedbacks::full_intra_request::FullIntraRequest;
use webrtc::rtcp::payload_feedbacks::picture_loss_indication::PictureLossIndication;
use webrtc::track::track_local::TrackLocal;
use webrtc::media::Sample;
use std::time::Duration;

pub struct WebRtcHost {
    pub pc: Arc<RTCPeerConnection>,
    pub video: Arc<TrackLocalStaticSample>,
    /// Unordered unreliable H.264 channel for browser WebCodecs (bypasses RTP JB).
    video_dc: Arc<RTCDataChannel>,
    video_seq: AtomicU32,
    video_w: AtomicU32,
    video_h: AtomicU32,
    pub pad_tx: mpsc::UnboundedSender<PadFrame>,
    offer_epoch: Arc<AtomicU64>,
    /// Set when a viewer reports it cannot decode and needs a fresh keyframe.
    keyframe_wanted: Arc<AtomicBool>,
}

impl WebRtcHost {
    /// True once since the last check: a viewer asked for a keyframe via RTCP.
    pub fn take_keyframe_request(&self) -> bool {
        self.keyframe_wanted.swap(false, Ordering::Relaxed)
    }

    pub async fn new(
        signal_out: mpsc::UnboundedSender<SignalMessage>,
        pad_device: Arc<Mutex<VirtualPad>>,
        as_bluetooth: bool,
        turn_url: Option<String>,
        turn_user: Option<String>,
        turn_pass: Option<String>,
        ice_ips: Vec<String>,
        offer_epoch: Arc<AtomicU64>,
    ) -> Result<(Self, mpsc::UnboundedReceiver<PadFrame>)> {
        let _ = as_bluetooth;
        let mut m = MediaEngine::default();
        m.register_default_codecs()?;
        // Ask Chrome to keep playout delay at 0 when we stamp RTP packets (gaming).
        m.register_header_extension(
            webrtc::rtp_transceiver::rtp_codec::RTCRtpHeaderExtensionCapability {
                uri: crate::latency::PLAYOUT_DELAY_URI.into(),
            },
            webrtc::rtp_transceiver::rtp_codec::RTPCodecType::Video,
            None,
        )?;
        let mut registry = Registry::new();
        registry = register_default_interceptors(registry, &mut m)?;
        let mut setting_engine = SettingEngine::default();
        let nat_ips: Vec<String> = ice_ips
            .into_iter()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();
        if !nat_ips.is_empty() {
            info!("ICE NAT 1:1 IPs: {nat_ips:?}");
            setting_engine.set_nat_1to1_ips(nat_ips, RTCIceCandidateType::Host);
        }
        // Offer a larger SCTP message size; we still fragment CLVD below the
        // common 64 KiB negotiated floor so Chrome peers always work.
        setting_engine.set_sctp_max_message_size_can_send(
            webrtc::api::setting_engine::SctpMaxMessageSize::Bounded(256 * 1024),
        );
        let api = APIBuilder::new()
            .with_setting_engine(setting_engine)
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
            // UDP + TCP: WSL / carrier NATs often need TCP TURN when UDP fails.
            let mut urls = vec![url.clone()];
            if !url.to_ascii_lowercase().contains("transport=tcp") {
                let sep = if url.contains('?') { '&' } else { '?' };
                urls.push(format!("{url}{sep}transport=tcp"));
            }
            info!("ICE TURN urls: {urls:?}");
            ice_servers.push(RTCIceServer {
                urls,
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

        let video = Arc::new(TrackLocalStaticSample::new(
            RTCRtpCodecCapability {
                mime_type: MIME_TYPE_H264.to_owned(),
                sdp_fmtp_line:
                    "level-asymmetry-allowed=1;packetization-mode=1;profile-level-id=42e01f"
                        .to_owned(),
                ..Default::default()
            },
            "video".to_owned(),
            "couchlink".to_owned(),
        ));
        let rtp_sender = pc
            .add_track(Arc::clone(&video) as Arc<dyn TrackLocal + Send + Sync>)
            .await?;

        // Nobody was reading RTCP, so every PLI a viewer sent was discarded and a
        // client that lost sync sat on a broken picture until the next scheduled
        // keyframe — up to IDR_INTERVAL of garbage. Watch for the standard
        // "send me a keyframe" feedback and answer it.
        let keyframe_wanted = Arc::new(AtomicBool::new(false));
        let kf = Arc::clone(&keyframe_wanted);
        tokio::spawn(async move {
            while let Ok((packets, _)) = rtp_sender.read_rtcp().await {
                for p in packets {
                    let any = p.as_any();
                    if any.downcast_ref::<PictureLossIndication>().is_some()
                        || any.downcast_ref::<FullIntraRequest>().is_some()
                    {
                        kf.store(true, Ordering::Relaxed);
                    }
                }
            }
        });

        let (pad_tx, pad_rx) = mpsc::unbounded_channel::<PadFrame>();
        let pad_tx_dc = pad_tx.clone();
        let pad_device_dc = Arc::clone(&pad_device);

        let pc2 = Arc::clone(&pc);
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

        // Pad: unordered + no retransmit — gaming input must never HOL-block.
        let pad_dc = pc2
            .create_data_channel(
                PAD_CHANNEL,
                Some(webrtc::data_channel::data_channel_init::RTCDataChannelInit {
                    ordered: Some(false),
                    max_retransmits: Some(0),
                    ..Default::default()
                }),
            )
            .await?;
        setup_pad_channel(pad_dc, pad_tx_dc, pad_device_dc).await;

        // Video: unordered, but allow a short retransmit window so fragmented
        // IDRs (often >64 KiB) are not permanently lost on a single drop.
        // Browser WebCodecs consumes this and skips Chrome's media JB.
        let video_dc = pc2
            .create_data_channel(
                VIDEO_CHANNEL,
                Some(webrtc::data_channel::data_channel_init::RTCDataChannelInit {
                    ordered: Some(false),
                    max_packet_life_time: Some(100),
                    ..Default::default()
                }),
            )
            .await?;
        let kf_dc = Arc::clone(&keyframe_wanted);
        setup_video_channel(Arc::clone(&video_dc), kf_dc).await;

        Ok((
            Self {
                pc,
                video,
                video_dc,
                video_seq: AtomicU32::new(0),
                video_w: AtomicU32::new(0),
                video_h: AtomicU32::new(0),
                pad_tx,
                offer_epoch,
                keyframe_wanted,
            },
            pad_rx,
        ))
    }

    /// Dimensions stamped into CLVD headers (from stream preset / capture).
    pub fn set_video_size(&self, width: u32, height: u32) {
        self.video_w.store(width, Ordering::Relaxed);
        self.video_h.store(height, Ordering::Relaxed);
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
        let epoch = self.offer_epoch.fetch_add(1, Ordering::SeqCst) + 1;
        signal_out.send(SignalMessage::Offer {
            sdp: local.sdp,
            epoch,
        })?;
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

    pub async fn push_h264(
        &self,
        annex_b: Vec<u8>,
        duration: Duration,
        keyframe: bool,
    ) -> Result<()> {
        use rtp::extension::playout_delay_extension::PlayoutDelayExtension;
        use rtp::extension::HeaderExtension;

        // DataChannel path first — browser WebCodecs paints without waiting on RTP JB.
        // Native clients ignore this channel and keep using the media track below.
        if self.video_dc.ready_state()
            == webrtc::data_channel::data_channel_state::RTCDataChannelState::Open
        {
            let seq = self.video_seq.fetch_add(1, Ordering::Relaxed);
            let w = self.video_w.load(Ordering::Relaxed).min(u32::from(u16::MAX)) as u16;
            let h = self.video_h.load(Ordering::Relaxed).min(u32::from(u16::MAX)) as u16;
            let au = VideoAccessUnit {
                seq,
                width: w,
                height: h,
                keyframe,
                annex_b: annex_b.clone(),
            };
            for frag in au.encode_fragments() {
                if let Err(e) = self.video_dc.send(&Bytes::from(frag)).await {
                    warn!("video datachannel send: {e}");
                    break;
                }
            }
        }

        // min=max=0 (in 10ms units) = play as soon as a full frame arrives.
        // Chrome treats this as a best-effort hint alongside jitterBufferTarget=0.
        let (min_delay, max_delay) = crate::latency::gaming_playout_delay();
        self.video
            .sample_writer()
            .with_extension(HeaderExtension::PlayoutDelay(PlayoutDelayExtension::new(
                min_delay, max_delay,
            )))
            .write_sample(&Sample {
                data: Bytes::from(annex_b),
                duration,
                ..Default::default()
            })
            .await?;
        Ok(())
    }
}

async fn setup_video_channel(dc: Arc<RTCDataChannel>, keyframe_wanted: Arc<AtomicBool>) {
    dc.on_open(Box::new(move || {
        info!("video datachannel open (CLVD → browser WebCodecs)");
        Box::pin(async {})
    }));
    dc.on_message(Box::new(move |msg: DataChannelMessage| {
        let keyframe_wanted = Arc::clone(&keyframe_wanted);
        Box::pin(async move {
            // Any inbound message = viewer lost sync / decoder reset.
            let _ = msg;
            keyframe_wanted.store(true, Ordering::Relaxed);
        })
    }));
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
