use anyhow::{Context, Result};
use bytes::BytesMut;
use std::sync::atomic::{AtomicUsize, Ordering};
use couchlink_proto::{PadFeedback, PadFrame, SignalMessage, PAD_CHANNEL};
use std::sync::Arc;
use tokio::sync::mpsc;
use tracing::{info, warn};

use crate::decode::{DecodedFrame, H264Decoder};

/// How many undecoded NALs may queue before the client gives up on catching up and
/// asks for a keyframe instead. A couple of frames absorbs normal jitter; more than
/// that is latency the viewer would never get back.
const MAX_NAL_BACKLOG: usize = 4;

/// Does this Annex-B buffer carry an SPS (7)?
///
/// Specifically SPS, not merely an IDR slice: an IDR without parameter sets is still
/// undecodable, so starting on one produces exactly the errors this gate exists to
/// avoid. The encoder emits SPS/PPS ahead of every IDR, so waiting for the SPS means
/// waiting for a complete, self-contained access unit.
fn starts_a_stream(nal: &[u8]) -> bool {
    let mut i = 0usize;
    while i + 3 < nal.len() {
        let (start, len) = if nal[i] == 0 && nal[i + 1] == 0 && nal[i + 2] == 1 {
            (i + 3, 3)
        } else if i + 4 < nal.len()
            && nal[i] == 0
            && nal[i + 1] == 0
            && nal[i + 2] == 0
            && nal[i + 3] == 1
        {
            (i + 4, 4)
        } else {
            i += 1;
            continue;
        };
        if nal[start] & 0x1F == 7 {
            return true;
        }
        i += len;
    }
    false
}
use crate::reachability;
use webrtc::api::interceptor_registry::register_default_interceptors;
use webrtc::api::media_engine::MediaEngine;
use webrtc::api::setting_engine::SettingEngine;
use webrtc::api::APIBuilder;
use webrtc::data_channel::RTCDataChannel;
use webrtc::ice_transport::ice_candidate_type::RTCIceCandidateType;
use webrtc::ice_transport::ice_server::RTCIceServer;
use webrtc::interceptor::registry::Registry;
use webrtc::peer_connection::configuration::RTCConfiguration;
use webrtc::peer_connection::sdp::session_description::RTCSessionDescription;
use webrtc::peer_connection::RTCPeerConnection;
use webrtc::rtcp::payload_feedbacks::picture_loss_indication::PictureLossIndication;

pub struct WebRtcPlayer {
    pub pc: Arc<RTCPeerConnection>,
    pub pad_dc: Arc<tokio::sync::Mutex<Option<Arc<RTCDataChannel>>>>,
    /// Host → player pad feedback (rumble / adaptive triggers / raw output).
    feedback_rx: tokio::sync::Mutex<Option<mpsc::UnboundedReceiver<PadFeedback>>>,
}

impl WebRtcPlayer {
    pub async fn new(
        signal_out: mpsc::UnboundedSender<SignalMessage>,
        turn_url: Option<String>,
        turn_user: Option<String>,
        turn_pass: Option<String>,
        ice_ips: Vec<String>,
    ) -> Result<(Self, mpsc::UnboundedReceiver<DecodedFrame>)> {
        let mut m = MediaEngine::default();
        m.register_default_codecs()?;
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
        let api = APIBuilder::new()
            .with_setting_engine(setting_engine)
            .with_media_engine(m)
            .with_interceptor_registry(registry)
            .build();
        // Public STUN for NAT discovery, plus the host's TURN relay (UDP + TCP)
        // for symmetric-NAT / CGNAT / WSL nested-NAT peers STUN alone can't punch.
        let mut ice_servers = vec![RTCIceServer {
            urls: vec![
                "stun:stun.l.google.com:19302".to_owned(),
                "stun:stun1.l.google.com:19302".to_owned(),
            ],
            ..Default::default()
        }];
        if let (Some(url), Some(user), Some(pass)) = (turn_url, turn_user, turn_pass) {
            let urls = reachability::expand_turn_urls(&url);
            info!("ICE TURN urls: {urls:?}");
            ice_servers.push(RTCIceServer {
                urls,
                username: user,
                credential: pass,
                ..Default::default()
            });
        } else {
            warn!("no TURN configured — remote/WSL peers may fail ICE without the host join URL's turn= params");
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

        let (feedback_tx, feedback_rx) = mpsc::unbounded_channel::<PadFeedback>();
        let pad_slot = Arc::clone(&pad_dc);
        pc.on_data_channel(Box::new(move |dc| {
            let pad_slot = Arc::clone(&pad_slot);
            let feedback_tx = feedback_tx.clone();
            Box::pin(async move {
                if dc.label() == PAD_CHANNEL {
                    info!("pad channel attached");
                    let fb_tx = feedback_tx.clone();
                    dc.on_message(Box::new(move |msg| {
                        let fb_tx = fb_tx.clone();
                        Box::pin(async move {
                            if !msg.is_string {
                                return;
                            }
                            let Ok(text) = std::str::from_utf8(&msg.data) else {
                                return;
                            };
                            if let Ok(fb) = serde_json::from_str::<PadFeedback>(text) {
                                let _ = fb_tx.send(fb);
                            }
                        })
                    }));
                    *pad_slot.lock().await = Some(dc);
                }
            })
        }));

        let (nal_tx, nal_rx) = mpsc::unbounded_channel::<bytes::Bytes>();
        let (frame_tx, frame_rx) = mpsc::unbounded_channel::<DecodedFrame>();

        // openh264 decodes in software, so it can fall behind a 60fps stream. This
        // channel is unbounded, and NALs cannot be dropped freely — a discarded
        // P-frame corrupts everything after it — so a backlog here becomes permanent
        // delay rather than a brief stutter. Track the depth so the reader can shed
        // safely (see below).
        let backlog = Arc::new(AtomicUsize::new(0));

        let backlog_dec = Arc::clone(&backlog);
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
                    backlog_dec.fetch_sub(1, Ordering::Relaxed);
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

        let pc_pli = Arc::clone(&pc);
        pc.on_track(Box::new(move |track, _, _| {
            let nal_tx = nal_tx.clone();
            let backlog = Arc::clone(&backlog);
            let pc_pli = Arc::clone(&pc_pli);
            Box::pin(async move {
                info!("video track received: {}", track.codec().capability.mime_type);
                // We joined mid-stream, so the next frames reference pictures we
                // never saw and carry no parameter sets. Ask for a keyframe straight
                // away instead of waiting for the sender's scheduled one — that wait
                // is seconds of undecodable video at startup.
                let _ = pc_pli
                    .write_rtcp(&[Box::new(PictureLossIndication {
                        sender_ssrc: 0,
                        media_ssrc: track.ssrc(),
                    })])
                    .await;
                let mut depacketizer = rtp::codecs::h264::H264Packet::default();
                depacketizer.is_avc = false; // false => depacketize() emits Annex-B (start-code) NALs
                let mut shedding = false;
                // Joining mid-stream means the frames arriving now reference pictures
                // we never saw. Feeding them to the decoder cannot produce a picture —
                // it only produces one error per frame until a keyframe lands. Wait
                // for something decodable instead.
                let mut have_keyframe = false;
                loop {
                    match track.read_rtp().await {
                        Ok((packet, _attrs)) => {
                            use rtp::packetizer::Depacketizer;
                            match depacketizer.depacketize(&packet.payload) {
                                Ok(nal) if !nal.is_empty() => {
                                    if !have_keyframe {
                                        if !starts_a_stream(&nal) {
                                            continue;
                                        }
                                        info!("keyframe received — starting decode");
                                        have_keyframe = true;
                                    }
                                    // Too far behind to catch up by decoding: shed
                                    // everything and ask the sender for a fresh
                                    // keyframe. Dropping alone would corrupt the
                                    // picture; dropping plus a PLI resynchronises on
                                    // the next frame. This is what the standard
                                    // feedback mechanism is for.
                                    if backlog.load(Ordering::Relaxed) > MAX_NAL_BACKLOG {
                                        // Shedding breaks the reference chain, so the
                                        // decoder cannot use anything until the next
                                        // keyframe. Re-arm the gate rather than feeding
                                        // it frames that can only fail.
                                        have_keyframe = false;
                                        if !shedding {
                                            warn!(
                                                "decoder {} frames behind — resyncing",
                                                MAX_NAL_BACKLOG
                                            );
                                            let _ = pc_pli
                                                .write_rtcp(&[Box::new(
                                                    PictureLossIndication {
                                                        sender_ssrc: 0,
                                                        media_ssrc: track.ssrc(),
                                                    },
                                                )])
                                                .await;
                                            shedding = true;
                                        }
                                        continue;
                                    }
                                    shedding = false;
                                    backlog.fetch_add(1, Ordering::Relaxed);
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

        Ok((
            Self {
                pc,
                pad_dc,
                feedback_rx: tokio::sync::Mutex::new(Some(feedback_rx)),
            },
            frame_rx,
        ))
    }

    pub async fn handle_offer(
        &self,
        sdp: String,
        offer_epoch: u64,
        signal_out: &mpsc::UnboundedSender<SignalMessage>,
    ) -> Result<()> {
        let offer = RTCSessionDescription::offer(sdp)?;
        self.pc.set_remote_description(offer).await?;
        let answer = self.pc.create_answer(None).await?;
        self.pc.set_local_description(answer).await?;
        let local = self.pc.local_description().await.context("local desc")?;
        signal_out.send(SignalMessage::Answer {
            sdp: local.sdp,
            epoch: offer_epoch,
        })?;
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

    /// Drain one host→player feedback message if present (non-blocking after take).
    pub async fn take_feedback_rx(&self) -> Option<mpsc::UnboundedReceiver<PadFeedback>> {
        self.feedback_rx.lock().await.take()
    }
}
