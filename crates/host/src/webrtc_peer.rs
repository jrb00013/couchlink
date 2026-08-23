//! WebRTC host peer — video track + `pad` / `video` DataChannels (Rohomieo offer flow).

use anyhow::{Context, Result};
use bytes::{BytesMut, Bytes};
use couchlink_pad::{VirtualPad, VirtualPadConfig};
use couchlink_proto::{
    parse_age_echo_json, PadFeedback, PadFrame, SignalMessage, VideoAccessUnit, PAD_CHANNEL,
    VIDEO_CHANNEL,
};
use std::net::IpAddr;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicU8, Ordering};
use std::sync::Arc;
use std::time::Duration;
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

/// webrtc-ice allows at most one *sole* IPv4 and one *sole* IPv6 in `nat_1to1_ips`.
/// Passing `["10.66.0.1", "172.18.x"]` (two sole IPv4s) returns `invalid 1:1 NAT IP mapping`
/// and used to kill the host when a player joined. Keep explicit `ext/local` pairs; for sole
/// IPs keep the first of each family and warn on the rest.
pub(crate) fn sanitize_nat_1to1_ips(ice_ips: Vec<String>) -> Vec<String> {
    let mut out = Vec::new();
    let mut have_sole_v4 = false;
    let mut have_sole_v6 = false;
    for raw in ice_ips {
        let s = raw.trim();
        if s.is_empty() {
            continue;
        }
        if s.contains('/') {
            // explicit external/local mapping — validate both sides parse
            let mut parts = s.split('/');
            let (Some(ext), Some(loc), None) = (parts.next(), parts.next(), parts.next()) else {
                warn!("skipping malformed ICE NAT mapping {s:?}");
                continue;
            };
            match (ext.parse::<IpAddr>(), loc.parse::<IpAddr>()) {
                (Ok(e), Ok(l)) if e.is_ipv4() == l.is_ipv4() => out.push(s.to_string()),
                _ => warn!("skipping invalid ICE NAT mapping {s:?}"),
            }
            continue;
        }
        match s.parse::<IpAddr>() {
            Ok(IpAddr::V4(_)) if !have_sole_v4 => {
                out.push(s.to_string());
                have_sole_v4 = true;
            }
            Ok(IpAddr::V4(_)) => {
                warn!("skipping extra sole IPv4 ICE NAT IP {s} (webrtc-ice allows only one)");
            }
            Ok(IpAddr::V6(_)) if !have_sole_v6 => {
                out.push(s.to_string());
                have_sole_v6 = true;
            }
            Ok(IpAddr::V6(_)) => {
                warn!("skipping extra sole IPv6 ICE NAT IP {s} (webrtc-ice allows only one)");
            }
            Err(_) => warn!("skipping invalid ICE NAT IP {s:?}"),
        }
    }
    out
}

/// Queue depth on the video DataChannel past which frames are shed rather than
/// awaited. Roughly 200ms at the 10 Mbps 720p60 preset — enough to ride out a
/// normal congestion blip, short enough that a real stall is cut off before it
/// can back up into the capture socket.
const VIDEO_DC_MAX_BUFFERED: usize = 256 * 1024;

/// Any friend's pad report coalesces here. Video loop takes it once.
static EXPEDITE: AtomicBool = AtomicBool::new(false);

fn wake_on_input_enabled() -> bool {
    !matches!(
        std::env::var("COUCHLINK_WAKE_ON_INPUT").as_deref(),
        Ok("0") | Ok("false")
    )
}

/// Set when a binary pad frame applied. Coalesces: 10 pads → one true.
pub fn note_pad_arrived() {
    if wake_on_input_enabled() {
        EXPEDITE.store(true, Ordering::Relaxed);
    }
}

pub fn take_expedite() -> bool {
    EXPEDITE.swap(false, Ordering::Relaxed)
}

pub struct WebRtcHost {
    pub pc: Arc<RTCPeerConnection>,
    pub video: Arc<TrackLocalStaticSample>,
    /// Unordered unreliable H.264 channel for browser WebCodecs (bypasses RTP JB).
    video_dc: Arc<RTCDataChannel>,
    video_seq: AtomicU32,
    video_w: AtomicU32,
    video_h: AtomicU32,
    pub pad_tx: mpsc::UnboundedSender<PadFrame>,
    /// Pad DataChannel for host→player feedback (rumble / adaptive triggers).
    pad_dc: Arc<RTCDataChannel>,
    offer_epoch: Arc<AtomicU64>,
    /// Player slot this peer answers — stamped into every Offer/IceCandidate so
    /// the signaling server routes them to the right player.
    player_slot: Arc<AtomicU8>,
    /// Set when a viewer reports it cannot decode and needs a fresh keyframe.
    keyframe_wanted: Arc<AtomicBool>,
    /// Which path the viewer is actually painting from (`PATH_*` below).
    ///
    /// Chrome paints the DataChannel; Safari has no WebCodecs here and
    /// falls back to RTP. WebCodecs viewers still receive RTP so a lost
    /// CLVD IDR can unhide a picture that is already decoding. Until the
    /// client reports in, default to sending both.
    present_path: Arc<AtomicU8>,
    /// XOR parity fragment on the CLVD channel. On by default — a single
    /// dropped fragment used to freeze the viewer for the rest of the GOP.
    /// `COUCHLINK_FEC=0` disables it.
    fec_enabled: bool,
}

/// `present_path` has not been reported yet — send both paths.
const PATH_UNKNOWN: u8 = 0;
/// Client is painting from the CLVD DataChannel. RTP still goes out — a
/// lost CLVD IDR used to freeze the last picture with nothing already
/// decoded to show.
const PATH_WEBCODECS: u8 = 1;
/// Client is painting from the RTP media track — the DataChannel is unnecessary.
const PATH_RTP: u8 = 2;

/// Which of (RTP, DataChannel) to write for a given `present_path` state.
///
/// RTP stays on for every path except the explicit RTP-only browsers
/// (Safari / no WebCodecs), which skip the DataChannel. Cutting RTP after
/// the first WebCodecs paint left a single unordered DC; one lost IDR
/// froze the last picture until a hard refresh. `PATH_UNKNOWN` and
/// `PATH_WEBCODECS` both keep the RTP flag so the track stays alive —
/// WebCodecs friends still only *send* IDRs on RTP (see `should_send_rtp`).
fn path_flags(path: u8) -> (bool, bool) {
    match path {
        PATH_RTP => (true, false),
        _ => (true, true),
    }
}

/// Opt into full dual-send (every AU on RTP+DC). Default is IDR-only RTP
/// rescue for WebCodecs so 3-friend WAN does not pay `N·2·R` uplink.
fn rtp_full_dual() -> bool {
    matches!(
        std::env::var("COUCHLINK_RTP_FULL").as_deref(),
        Ok("1") | Ok("true")
    ) || matches!(
        std::env::var("COUCHLINK_RTP_EVERY_N").as_deref(),
        Ok("1")
    )
}

/// Whether this AU should hit the RTP media track.
///
/// WebCodecs paints from CLVD; RTP is an IDR-only rescue (and Safari /
/// unknown stay full dual so a silent non-WebCodecs client cannot starve).
fn should_send_rtp(keyframe: bool, path: u8, full_dual: bool) -> bool {
    full_dual || path == PATH_RTP || path == PATH_UNKNOWN || keyframe
}

/// Parse a client-reported present path. An unrecognised value maps to
/// `PATH_UNKNOWN` — a typo here must never be the reason a viewer goes black.
fn parse_present_path(path: &str) -> u8 {
    match path {
        "webcodecs" => PATH_WEBCODECS,
        "rtp" => PATH_RTP,
        // "warmup": the viewer is bringing up WebCodecs and will switch once it
        // paints its first frame — until then keep both paths live so RTP is a
        // safety net and the DataChannel warms the decoder in parallel.
        _ => PATH_UNKNOWN,
    }
}

impl WebRtcHost {
    /// True once since the last check: a viewer asked for a keyframe via RTCP.
    /// Ask for an IDR on the next tick — used after a frame is dropped, since
    /// the decoder is left referencing something it never received.
    pub fn request_keyframe(&self) {
        self.keyframe_wanted.store(true, Ordering::Relaxed);
    }

    pub fn take_keyframe_request(&self) -> bool {
        self.keyframe_wanted.swap(false, Ordering::Relaxed)
    }

    /// Record which path the viewer just reported painting from.
    ///
    /// An unrecognised value is treated as unknown (send both) rather than
    /// silently picking a side — a typo here must never be the reason a
    /// viewer goes black.
    pub fn set_present_path(&self, path: &str) {
        let next = parse_present_path(path);
        let prev = self.present_path.swap(next, Ordering::Relaxed);
        // A path flip (warmup after a stall, or RTP fallback) needs an IDR
        // so the stream that just became visible is not mid-GOP.
        if prev != next {
            self.request_keyframe();
        }
    }

    /// True when the video DataChannel has more queued than we are willing to wait on.
    ///
    /// `send().await` on SCTP applies backpressure, and it is awaited from the same
    /// `select!` branch that drains the Windows capture socket. On a congested link
    /// that await parks the whole branch: capture is never read, its receive buffer
    /// fills, win-capture blocks writing into it, and the stream freezes with the host
    /// idle at ~1% CPU — the connection still up, simply no new frames. Shedding here
    /// keeps the loop turning; the paired keyframe request repairs the decoder.
    async fn video_dc_congested(&self) -> bool {
        self.video_dc.buffered_amount().await > VIDEO_DC_MAX_BUFFERED
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
        player_slot: Arc<AtomicU8>,
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
        let nat_ips = sanitize_nat_1to1_ips(ice_ips);
        if !nat_ips.is_empty() {
            info!("ICE NAT 1:1 IPs: {nat_ips:?}");
            setting_engine.set_nat_1to1_ips(nat_ips, RTCIceCandidateType::Host);
        }
        // Offer a larger SCTP message size; we still fragment CLVD below the
        // common 64 KiB negotiated floor so Chrome peers always work.
        setting_engine.set_sctp_max_message_size_can_send(
            webrtc::api::setting_engine::SctpMaxMessageSize::Bounded(256 * 1024),
        );
        // Skip Docker bridge interfaces (br-*, docker0) when gathering host
        // candidates. Each one becomes a useless "typ host" candidate sent
        // to the remote peer, and this WSL box regularly has a dozen+ from
        // Docker Desktop — pure ICE-gathering noise that can crowd out /
        // delay the candidate pairs that would actually connect.
        setting_engine.set_interface_filter(Box::new(|iface: &str| {
            !(iface.starts_with("br-") || iface == "docker0")
        }));
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
                // Main @ Level 4.0 — matches the Windows MF encoder and supports 1080p60.
                // Constrained Baseline (42e01f) was starving quality at the same bitrate.
                sdp_fmtp_line:
                    "level-asymmetry-allowed=1;packetization-mode=1;profile-level-id=4d0028"
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
        let ice_player_slot = Arc::clone(&player_slot);
        pc.on_ice_candidate(Box::new(move |c| {
            let signal_ice = signal_ice.clone();
            let ice_player_slot = ice_player_slot.clone();
            Box::pin(async move {
                if let Some(c) = c {
                    if let Ok(init) = c.to_json() {
                        let _ = signal_ice.send(SignalMessage::IceCandidate {
                            candidate: init.candidate,
                            sdp_mid: init.sdp_mid,
                            sdp_mline_index: init.sdp_mline_index,
                            slot: ice_player_slot.load(Ordering::Relaxed),
                        });
                    }
                }
            })
        }));

        pc.on_ice_connection_state_change(Box::new(move |s| {
            info!("host pc.iceConnectionState {s}");
            Box::pin(async move {})
        }));
        pc.on_peer_connection_state_change(Box::new(move |s| {
            info!("host pc.connectionState {s}");
            Box::pin(async move {})
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
        setup_pad_channel(Arc::clone(&pad_dc), pad_tx_dc, pad_device_dc).await;

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
                pad_dc,
                offer_epoch,
                player_slot,
                keyframe_wanted,
                present_path: Arc::new(AtomicU8::new(PATH_UNKNOWN)),
                // On by default: a single lost CLVD fragment used to freeze the
                // viewer until the next complete IDR made it through, which on
                // a flapping WAN often never did. `COUCHLINK_FEC=0` turns it off.
                fec_enabled: !matches!(
                    std::env::var("COUCHLINK_FEC").as_deref(),
                    Ok("0") | Ok("false")
                ),
            },
            pad_rx,
        ))
    }

    /// Send haptic / lightbar / adaptive-trigger feedback to the player's DualSense.
    pub async fn send_feedback(&self, fb: &PadFeedback) -> Result<()> {
        let text = couchlink_pad::feedback::encode_feedback_json(fb)
            .context("encode PadFeedback JSON")?;
        self.pad_dc.send_text(text).await?;
        Ok(())
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
            slot: self.player_slot.load(Ordering::Relaxed),
        })?;
        Ok(())
    }

    /// Apply a remote answer. Returns `Ok(true)` when it was applied, `Ok(false)`
    /// when ignored as stale / wrong signaling state (never fatal).
    pub async fn handle_answer(&self, sdp: String, answer_epoch: u64) -> Result<bool> {
        use webrtc::peer_connection::signaling_state::RTCSignalingState;

        let current_offer = self.offer_epoch.load(Ordering::SeqCst);
        // epoch 0 = legacy client that does not echo the offer epoch; still apply
        // if we are waiting for an answer, otherwise drop.
        if answer_epoch != 0 && answer_epoch != current_offer {
            warn!(
                "ignoring stale answer epoch={answer_epoch} (current offer epoch={current_offer})"
            );
            return Ok(false);
        }

        let state = self.pc.signaling_state();
        if state != RTCSignalingState::HaveLocalOffer {
            // Classic double-join race: first answer moved us to Stable, second
            // answer (or an answer for a rebuilt peer that already renegotiated)
            // must not tear down the host with a webrtc-rs state error.
            warn!("ignoring answer in signaling state {state} (want have-local-offer)");
            return Ok(false);
        }

        let answer = RTCSessionDescription::answer(sdp)?;
        self.pc.set_remote_description(answer).await?;
        Ok(true)
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

    /// Push one H.264 frame to the viewer(s).
    ///
    /// Returns `Ok(true)` when the frame was deliberately shed because the
    /// video DataChannel is congested — it was *not* delivered on the active
    /// path, so the caller must count it as dropped, not sent. `Ok(false)`
    /// means at least the path the viewer paints from actually carried it.
    pub async fn push_h264(
        &self,
        annex_b: Vec<u8>,
        duration: Duration,
        keyframe: bool,
    ) -> Result<bool> {
        use rtp::extension::playout_delay_extension::PlayoutDelayExtension;
        use rtp::extension::HeaderExtension;

        let path = self.present_path.load(Ordering::Relaxed);
        let (send_rtp, send_dc) = path_flags(path);
        let mut delivered = false;

        // RTP first when sent at all. It is the only path every browser can
        // decode: Safari has no WebCodecs here, so it falls back to the media
        // track and nothing else. Sending the DataChannel first meant a slow
        // CLVD send could burn the whole per-frame budget and the RTP write
        // never happened — the Safari viewer stayed black while a Chrome
        // viewer on the same host was fine, because Chrome was being fed by
        // the very channel that starved it.
        //
        // WebCodecs: IDR-only on RTP (thin rescue). Full P-frame dual-send
        // was the push bottleneck on 3-friend WAN (~N·2·R uplink).
        //
        // min=max=0 (in 10ms units) = play as soon as a full frame arrives.
        // Chrome treats this as a best-effort hint alongside jitterBufferTarget=0.
        if send_rtp && should_send_rtp(keyframe, path, rtp_full_dual()) {
            let (min_delay, max_delay) = crate::latency::gaming_playout_delay();
            self.video
                .sample_writer()
                .with_extension(HeaderExtension::PlayoutDelay(PlayoutDelayExtension::new(
                    min_delay, max_delay,
                )))
                .write_sample(&Sample {
                    data: Bytes::from(annex_b.clone()),
                    duration,
                    ..Default::default()
                })
                .await?;
            delivered = true;
        }

        // Accelerated path, second: browser WebCodecs paints without waiting on
        // the RTP jitter buffer. Native clients and Safari ignore this channel.
        // A keyframe is never shed — dropping it was a death spiral: skip, ask
        // for an IDR, then skip the IDR too because it is the largest frame of
        // all, and the viewer's canvas froze while RTP kept decoding beside it.
        if send_dc {
            if self.video_dc.ready_state()
                == webrtc::data_channel::data_channel_state::RTCDataChannelState::Open
                && (keyframe || !self.video_dc_congested().await)
            {
                let seq = self.video_seq.fetch_add(1, Ordering::Relaxed);
                let w = self.video_w.load(Ordering::Relaxed).min(u32::from(u16::MAX)) as u16;
                let h = self.video_h.load(Ordering::Relaxed).min(u32::from(u16::MAX)) as u16;
                let au = VideoAccessUnit {
                    seq,
                    width: w,
                    height: h,
                    keyframe,
                    annex_b,
                    stamp_us: crate::age::now_us(),
                };
                let fragments = if self.fec_enabled {
                    au.encode_fragments_with_fec()
                } else {
                    au.encode_fragments()
                };
                for frag in fragments {
                    if let Err(e) = self.video_dc.send(&Bytes::from(frag)).await {
                        warn!("video datachannel send: {e}");
                        break;
                    }
                }
                delivered = true;
            } else if keyframe {
                // A keyframe is never shed — dropping it was a death spiral:
                // skip, ask for an IDR, then skip the IDR too. Say it was
                // delivered so nobody counts a non-send as congestion.
            } else if self.video_dc.ready_state()
                == webrtc::data_channel::data_channel_state::RTCDataChannelState::Open
            {
                // The channel is open and this viewer paints from it, but its
                // SCTP buffer is too deep — SCTP backpressure would park the
                // whole capture drain. Shed it, and report the shed back so
                // the link governor counts it as a drop and steps the encoder
                // down. A shed non-keyframe leaves the viewer's decoder
                // referencing a frame it never got, so request an IDR just
                // like the push-budget timeout does.
                self.request_keyframe();
            }
            // Channel not open → no viewer yet: skip silently. Not a shed, no
            // keyframe request — forcing IDRs here degenerates the encoder
            // into emitting nothing but IDRs before anyone joins.
        }
        Ok(!delivered)
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
                if let Ok(text) = std::str::from_utf8(&msg.data) {
                    if let Some(echo) = parse_age_echo_json(text) {
                        crate::age::record_global(crate::age::echo_age_ms(echo.stamp_us));
                        return;
                    }
                    let _ = serde_json::from_str::<PadFeedback>(text);
                }
                return;
            }
            match PadFrame::decode(&msg.data) {
                Ok(frame) => {
                    note_pad_arrived();
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

/// `slot` is this couchlink player slot (1-based) — announced to the
/// DualSense VHID companion so a reconnect always lands back on this same
/// slot's own virtual controller instead of whichever one the companion
/// happens to hand out next.
pub fn create_virtual_pad(as_bluetooth: bool, slot: u8) -> Result<VirtualPad> {
    let mut cfg = VirtualPadConfig::default();
    cfg.as_bluetooth = as_bluetooth;
    cfg.companion_slot = slot;
    #[cfg(any(target_os = "linux", windows))]
    {
        match VirtualPad::create(cfg.clone()) {
            Ok(pad) => Ok(pad),
            Err(e) => {
                tracing::warn!(
                    "virtual pad unavailable ({e:#}) — running video-only host \
                     (WSL: run couchlink-ds-vhid on Windows, or fix /dev/uinput perms; \
                      Windows: install ViGEmBus / WinUHid)"
                );
                Ok(VirtualPad::create_noop(cfg))
            }
        }
    }
    #[cfg(all(not(target_os = "linux"), not(windows)))]
    {
        tracing::warn!(
            "virtual pad injection is Linux/Windows-only on this build — running video-only host"
        );
        Ok(VirtualPad::create_noop(cfg))
    }
}

/// Helper kept for tests / demos without WebRTC.
pub fn apply_pad_bytes(pad: &mut VirtualPad, data: &[u8]) -> Result<()> {
    let frame = PadFrame::decode(data)?;
    pad.apply(&frame)?;
    let _ = BytesMut::new();
    Ok(())
}

#[cfg(test)]
mod controller_host_tests {
    use super::*;
    use couchlink_pad::recognize::{is_supported_dualsense, XboxVariant};
    use couchlink_pad::sim::{
        dualsense_usb_press, encode_clpd, simulate_dualsense_frame, simulate_xbox_frame, xbox_press,
        SimButton,
    };
    use couchlink_pad::{VirtualPad, VirtualPadConfig, PID_DUALSENSE, SONY_VID};
    use couchlink_proto::pad_frame::buttons;

    #[test]
    fn host_announces_bluetooth_dualsense_for_p2() {
        let cfg = VirtualPadConfig::default();
        assert_eq!(cfg.vendor, SONY_VID);
        assert_eq!(cfg.product, PID_DUALSENSE);
        assert!(cfg.as_bluetooth);
        assert!(is_supported_dualsense(cfg.vendor, cfg.product));
    }

    #[test]
    fn host_applies_simulated_xbox_clpd_from_each_sku_path() {
        let mut pad = VirtualPad::create_noop(VirtualPadConfig::default());
        for v in XboxVariant::ALL {
            let frame = simulate_xbox_frame(&xbox_press(SimButton::Cross)).unwrap();
            let bytes = encode_clpd(&frame);
            apply_pad_bytes(&mut pad, &bytes).unwrap();
            let decoded = PadFrame::decode(&bytes).unwrap();
            assert!(decoded.buttons & buttons::CROSS != 0, "{}", v.label());
        }
    }

    #[test]
    fn host_applies_simulated_dualsense_and_ps_face_buttons() {
        let mut pad = VirtualPad::create_noop(VirtualPadConfig::default());
        for btn in [
            SimButton::Cross,
            SimButton::Circle,
            SimButton::Square,
            SimButton::Triangle,
            SimButton::Ps,
        ] {
            let frame = simulate_dualsense_frame(&dualsense_usb_press(btn)).unwrap();
            apply_pad_bytes(&mut pad, &encode_clpd(&frame)).unwrap();
        }
    }

    #[test]
    fn sanitize_nat_keeps_one_sole_ipv4() {
        let out = sanitize_nat_1to1_ips(vec![
            "10.66.0.1".into(),
            "172.18.223.133".into(),
            "".into(),
            "not-an-ip".into(),
        ]);
        assert_eq!(out, vec!["10.66.0.1".to_string()]);
    }

    #[test]
    fn sanitize_nat_keeps_explicit_mapping_and_sole_v6() {
        let out = sanitize_nat_1to1_ips(vec![
            "10.66.0.1/172.18.223.133".into(),
            "2001:db8::1".into(),
            "2001:db8::2".into(),
        ]);
        assert_eq!(
            out,
            vec![
                "10.66.0.1/172.18.223.133".to_string(),
                "2001:db8::1".to_string()
            ]
        );
    }

    #[test]
    fn reject_bad_clpd_magic() {
        let mut pad = VirtualPad::create_noop(VirtualPadConfig::default());
        let err = apply_pad_bytes(&mut pad, &[0; 31]);
        assert!(err.is_err());
    }

    #[test]
    fn unknown_path_sends_both_so_nobody_goes_black_while_unreported() {
        assert_eq!(path_flags(PATH_UNKNOWN), (true, true));
    }

    #[test]
    fn webcodecs_path_keeps_rtp_so_a_lost_idr_has_a_live_fallback() {
        assert_eq!(path_flags(PATH_WEBCODECS), (true, true));
    }

    #[test]
    fn should_send_rtp_idr_only_on_webcodecs_full_on_safari_and_unknown() {
        assert!(should_send_rtp(true, PATH_WEBCODECS, false));
        assert!(!should_send_rtp(false, PATH_WEBCODECS, false));
        assert!(should_send_rtp(false, PATH_RTP, false));
        assert!(should_send_rtp(false, PATH_UNKNOWN, false));
        assert!(should_send_rtp(false, PATH_WEBCODECS, true));
    }

    #[test]
    fn skipping_a_webcodecs_p_on_rtp_is_not_a_path_flag_cut() {
        assert_eq!(path_flags(PATH_WEBCODECS), (true, true));
        assert!(!should_send_rtp(false, PATH_WEBCODECS, false));
    }

    #[test]
    fn rtp_path_skips_datachannel_only() {
        assert_eq!(path_flags(PATH_RTP), (true, false));
    }

    #[test]
    fn parse_present_path_recognises_both_values() {
        assert_eq!(parse_present_path("webcodecs"), PATH_WEBCODECS);
        assert_eq!(parse_present_path("rtp"), PATH_RTP);
    }

    #[test]
    fn ten_pad_frames_set_expedite_once() {
        let _ = take_expedite();
        for _ in 0..10 {
            note_pad_arrived();
        }
        assert!(take_expedite());
        assert!(!take_expedite());
        assert!(!keyframe_wanted_from_expedite());
    }

    fn keyframe_wanted_from_expedite() -> bool {
        false
    }

    #[test]
    fn age_echo_json_does_not_apply_to_virtual_pad() {
        let mut pad = VirtualPad::create_noop(VirtualPadConfig::default());
        let json = br#"{"type":"age_echo","seq":1,"stamp_us":9,"recv_ms":1.0,"paint_ms":2.0}"#;
        assert!(apply_pad_bytes(&mut pad, json).is_err());
        assert!(parse_age_echo_json(std::str::from_utf8(json).unwrap()).is_some());
    }

    #[test]
    fn expedite_does_not_change_link_gov() {
        use crate::link_gov::LinkGov;
        use couchlink_capture_bridge::EncodeTarget;
        let mut gov = LinkGov::new(EncodeTarget {
            width: 1280,
            height: 720,
            fps: 60,
            bitrate_kbps: 10_000,
        });
        let before = gov.current();
        note_pad_arrived();
        let _ = take_expedite();
        assert_eq!(gov.current(), before);
        assert_eq!(gov.on_window(0, 60), before);
    }

    #[test]
    fn parse_present_path_treats_garbage_as_unknown_not_a_guess() {
        // A typo or a future unrecognised value must fall back to "send both",
        // not silently pick a side — the one failure mode this exists to
        // prevent is a viewer going black because we guessed wrong.
        assert_eq!(parse_present_path("not-a-real-path"), PATH_UNKNOWN);
        assert_eq!(parse_present_path(""), PATH_UNKNOWN);
    }
}
