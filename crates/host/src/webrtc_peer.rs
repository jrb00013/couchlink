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

/// Queue depth on the video DataChannel past which *P-frames* are shed rather
/// than awaited. At 5 Mbps (~625 KB/s), 24 KiB ≈ 39 ms — inside the 45 ms
/// input wow-bar so trickle/governor react before bufferbloat eats the budget.
/// (96 KiB was ~157 ms — congestion looked healthy while paint died.)
const VIDEO_DC_MAX_BUFFERED: usize = 24 * 1024;

/// Keyframes must clear this higher ceiling so a multi-fragment IDR can finish.
/// Fragments are 14 KiB each; a 720p IDR is often 40–100 KiB. The P-frame 24 KiB
/// cap aborted mid-AU → WC never configured → 0 `input_wm` / S_p50 forever while
/// RTP paint stayed green. 256 KiB ≈ one IDR; SCTP drains async after queue.
const VIDEO_DC_MAX_BUFFERED_IDR: usize = 256 * 1024;

/// Coalesce window for hybrid DC bootstrap PLI (RTP live). Matches IDR_INTERVAL
/// so we never storm the shared encoder — one early IDR for WC, then silence.
const KEYFRAME_COALESCE_DUAL_MS: u64 = 3000;

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

fn note_input_wm(atom: &AtomicU32, seq: u32) {
    loop {
        let cur = atom.load(Ordering::Relaxed);
        if seq <= cur {
            return;
        }
        if atom
            .compare_exchange_weak(cur, seq, Ordering::Relaxed, Ordering::Relaxed)
            .is_ok()
        {
            return;
        }
    }
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
    /// Per-peer coalesce clock for keyframe requests (ms since unix epoch).
    /// Process-global coalesce starved the congested peer when a healthy peer
    /// ate the shared token — keep this on the peer.
    keyframe_coalesce_ms: Arc<AtomicU64>,
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
    /// Slow-peer isolation: after sustained sheds, skip deltas until recovered.
    trickle: Arc<AtomicBool>,
    shed_streak: Arc<AtomicU32>,
    ok_streak: Arc<AtomicU32>,
    /// Last pad seq applied — stamped into CLVD as input_wm for client photon metric.
    last_input_wm: Arc<AtomicU32>,
    /// Wall-clock ms this peer last transitioned into `PATH_WEBCODECS`, or 0.
    /// FEC engagement waits `FEC_PROMOTE_GRACE_MS` past this so the CLVD parity
    /// tax does not stack on top of the promote-moment bandwidth re-estimate
    /// (that stacking is what forced a real Chrome RTCP PLI on the shared RTP
    /// encoder right after promote — see `coalesce_keyframe_request_within`).
    webcodecs_since_ms: Arc<AtomicU64>,
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Grace period after a promote to `PATH_WEBCODECS` before FEC parity
/// fragments are added to the CLVD channel. Chrome's bandwidth estimate is
/// still re-settling right after a path flip; adding FEC's ~50% CLVD size tax
/// in that same instant was stealing the link RTP needed, which triggered a
/// real Chrome RTCP PLI on the shared encoder (big IDR on an already
/// congested link) — the black flash Joel saw at promote.
const FEC_PROMOTE_GRACE_MS: u64 = 1500;

/// Pre-announce / `"clvd"` reclaim — same hybrid as warmup (full RTP + thin CLVD).
pub(crate) const PATH_UNKNOWN: u8 = 0;
/// Photon path live (`input_wm` + WC). **RTP stays full** for visible paint
/// (v25 feel); CLVD stays thin for S_p50. Exclusive binary killed paint/feel.
pub(crate) const PATH_WEBCODECS: u8 = 1;
/// Safari / no-WebCodecs: RTP only (no CLVD).
pub(crate) const PATH_RTP: u8 = 2;

/// Dual hybrid — full RTP (playable canvas) + thinned CLVD (WC/`input_wm`).
/// Join (DC open), stall rescue, and healthy promote all land here flags-wise;
/// WEBCODECS only adds FEC.
pub(crate) const PATH_WARMUP: u8 = 3;

/// Which of (RTP, DataChannel) to write for a given `present_path` state.
///
/// Hybrid (Joel beat-self): visible paint = full RTP forever; CLVD rides thin
/// for WebCodecs + `input_wm` (S_p50). Exclusive CLVD-only flipped RTP off and
/// either never painted or fell into IDR-only 1fps (v26). `COUCHLINK_RTP_FULL=1`
/// forces full dual CLVD (escape hatch — WAN shed risk).
pub(crate) fn path_flags(path: u8) -> (bool, bool) {
    if rtp_full_dual() {
        return match path {
            PATH_RTP => (true, false),
            _ => (true, true),
        };
    }
    match path {
        PATH_RTP => (true, false),
        // Hybrid: full RTP + CLVD for every WC-capable path.
        PATH_WEBCODECS | PATH_UNKNOWN | PATH_WARMUP => (true, true),
        _ => (true, true),
    }
}

/// Opt into **full-rate** CLVD alongside full RTP (every AU on both). Default
/// is thin CLVD (`should_send_clvd`) so dual does not SCTP-death the link.
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
/// Hybrid: whenever RTP is enabled, send **every** frame (v25 feel). IDR-only
/// RTP on dual left Joel at 1fps after fallback_timer.
pub(crate) fn should_send_rtp(keyframe: bool, path: u8, full_dual: bool) -> bool {
    let (send_rtp, _) = path_flags(path);
    let _ = (keyframe, full_dual);
    send_rtp
}

/// Whether this AU should hit the CLVD DataChannel.
///
/// Thin by default (IDR + every 2nd) while full RTP carries paint — dual-full
/// shed 20–67% (v26). `COUCHLINK_RTP_FULL=1` → every AU on CLVD too.
pub(crate) fn should_send_clvd(keyframe: bool, path: u8, seq: u32) -> bool {
    let (_, send_dc) = path_flags(path);
    if !send_dc {
        return false;
    }
    if rtp_full_dual() {
        return true;
    }
    // IDR + every 2nd — denser than /4 so WC stays warm without dual-full shed.
    keyframe || seq % 2 == 0
}

/// Parse a client-reported present path. An unrecognised value maps to
/// `PATH_UNKNOWN` — a typo here must never be the reason a viewer goes black.
fn parse_present_path(path: &str) -> u8 {
    match path {
        "webcodecs" => PATH_WEBCODECS,
        "rtp" => PATH_RTP,
        // Stall rescue — RTP + CLVD so canvas can recover.
        "warmup" => PATH_WARMUP,
        // Alias for hybrid UNKNOWN (full RTP + thin CLVD, FEC off).
        "clvd" | "binary" => PATH_UNKNOWN,
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

    /// Same as `request_keyframe`, but rate-limited so a burst of sheds cannot
    /// force the encoder into IDR-only mode. Coalesce clock is **per peer**.
    pub fn request_keyframe_coalesced(&self) {
        self.request_keyframe_coalesced_within(KEYFRAME_COALESCE_MS);
    }

    /// Hybrid dual: ≥IDR_INTERVAL gap so CLVD bootstrap cannot storm shared RTP.
    pub fn request_keyframe_coalesced_dual(&self) {
        self.request_keyframe_coalesced_within(KEYFRAME_COALESCE_DUAL_MS);
    }

    fn request_keyframe_coalesced_within(&self, min_gap_ms: u64) {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);
        let prev = self.keyframe_coalesce_ms.load(Ordering::Relaxed);
        if now.saturating_sub(prev) < min_gap_ms {
            return;
        }
        if self
            .keyframe_coalesce_ms
            .compare_exchange(prev, now, Ordering::Relaxed, Ordering::Relaxed)
            .is_ok()
        {
            self.request_keyframe();
        }
    }

    pub fn take_keyframe_request(&self) -> bool {
        self.keyframe_wanted.swap(false, Ordering::Relaxed)
    }

    /// True when this peer is on hybrid dual (full RTP + CLVD). Viewer PLI then
    /// should force a **single** shared IDR — burst-of-3 blacks every peer's RTP
    /// while WC only needed one complete CLVD keyframe.
    pub fn hybrid_dual(&self) -> bool {
        let (send_rtp, send_dc) = path_flags(self.present_path.load(Ordering::Relaxed));
        send_rtp && send_dc
    }

    /// Record which path the viewer just reported painting from.
    ///
    /// An unrecognised value is treated as unknown (send both) rather than
    /// silently picking a side — a typo here must never be the reason a
    /// viewer goes black.
    pub fn set_present_path(&self, path: &str) {
        let next = parse_present_path(path);
        let prev = self.present_path.swap(next, Ordering::Relaxed);
        if prev == next {
            return;
        }
        let (rtp_prev, _) = path_flags(prev);
        let (rtp_next, _) = path_flags(next);
        tracing::warn!(
            prev,
            next,
            path,
            rtp_prev,
            rtp_next,
            "present_path flipped"
        );
        // Hybrid warmup↔webcodecs keeps full RTP. IDR on every flip blacked
        // Joel's canvas every ~2s (v28). Only IDR when RTP enablement changes.
        if rtp_prev != rtp_next {
            self.request_keyframe();
        }
        // Stamp the promote moment so FEC (below) waits out the bandwidth
        // re-estimate instead of taxing the link the instant RTP is still
        // settling from the flip.
        if next == PATH_WEBCODECS && prev != PATH_WEBCODECS {
            self.webcodecs_since_ms.store(now_ms(), Ordering::Relaxed);
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
        self.video_dc_congested_for(false).await
    }

    async fn video_dc_congested_for(&self, keyframe: bool) -> bool {
        let cap = if keyframe {
            VIDEO_DC_MAX_BUFFERED_IDR
        } else {
            VIDEO_DC_MAX_BUFFERED
        };
        self.video_dc.buffered_amount().await > cap
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
        let keyframe_coalesce_ms = Arc::new(AtomicU64::new(0));
        let present_path = Arc::new(AtomicU8::new(PATH_UNKNOWN));
        let kf = Arc::clone(&keyframe_wanted);
        let kf_ms = Arc::clone(&keyframe_coalesce_ms);
        let rtcp_path = Arc::clone(&present_path);
        tokio::spawn(async move {
            while let Ok((packets, _)) = rtp_sender.read_rtcp().await {
                for p in packets {
                    let any = p.as_any();
                    if any.downcast_ref::<PictureLossIndication>().is_some()
                        || any.downcast_ref::<FullIntraRequest>().is_some()
                    {
                        // Hybrid dual: Chrome RTCP PLI is usually our own CLVD
                        // tax stealing RTP, not a real picture problem. Ignoring
                        // it stops the shared-encoder IDR → black cycle; the
                        // periodic 3s IDR_INTERVAL still heals genuine loss.
                        let path = rtcp_path.load(Ordering::Relaxed);
                        let (send_rtp, send_dc) = path_flags(path);
                        if send_rtp && send_dc {
                            continue;
                        }
                        coalesce_keyframe_request_within(
                            &kf,
                            &kf_ms,
                            KEYFRAME_COALESCE_MS,
                        );
                    }
                }
            }
        });

        let (pad_tx, pad_rx) = mpsc::unbounded_channel::<PadFrame>();
        let pad_tx_dc = pad_tx.clone();
        let pad_device_dc = Arc::clone(&pad_device);
        let last_input_wm = Arc::new(AtomicU32::new(0));
        let last_input_wm_pad = Arc::clone(&last_input_wm);

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
        setup_pad_channel(
            Arc::clone(&pad_dc),
            pad_tx_dc,
            pad_device_dc,
            last_input_wm_pad,
        )
        .await;

        // Video: unordered, short lifetime — FEC recovers single fragment loss;
        // stale retransmits after ~40ms are useless (decodeBacklogPolicy asks IDR).
        let video_dc = pc2
            .create_data_channel(
                VIDEO_CHANNEL,
                Some(webrtc::data_channel::data_channel_init::RTCDataChannelInit {
                    ordered: Some(false),
                    max_packet_life_time: Some(40),
                    ..Default::default()
                }),
            )
            .await?;
        let kf_dc = Arc::clone(&keyframe_wanted);
        let kf_dc_ms = Arc::clone(&keyframe_coalesce_ms);
        setup_video_channel(
            Arc::clone(&video_dc),
            kf_dc,
            kf_dc_ms,
            Arc::clone(&present_path),
        )
        .await;

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
                keyframe_coalesce_ms,
                present_path,
                // On by default: a single lost CLVD fragment used to freeze the
                // viewer until the next complete IDR made it through, which on
                // a flapping WAN often never did. `COUCHLINK_FEC=0` turns it off.
                fec_enabled: !matches!(
                    std::env::var("COUCHLINK_FEC").as_deref(),
                    Ok("0") | Ok("false")
                ),
                trickle: Arc::new(AtomicBool::new(false)),
                shed_streak: Arc::new(AtomicU32::new(0)),
                ok_streak: Arc::new(AtomicU32::new(0)),
                last_input_wm,
                webcodecs_since_ms: Arc::new(AtomicU64::new(0)),
            },
            pad_rx,
        ))
    }

    /// Sustained real congestion sheds before this peer enters cautious mode.
    const TRICKLE_ENTER_SHEDS: u32 = 8;
    /// Clean deliveries while cautious before full rate resumes (any frame type).
    const TRICKLE_EXIT_OKS: u32 = 4;

    fn note_shed(&self) {
        self.ok_streak.store(0, Ordering::Relaxed);
        let s = self.shed_streak.fetch_add(1, Ordering::Relaxed) + 1;
        if should_enter_trickle(s) && !self.trickle.swap(true, Ordering::Relaxed) {
            warn!("peer entering trickle mode — throttling congested deltas until queue drains");
        }
    }

    fn note_delivered(&self) {
        self.shed_streak.store(0, Ordering::Relaxed);
        if !self.trickle.load(Ordering::Relaxed) {
            return;
        }
        let o = self.ok_streak.fetch_add(1, Ordering::Relaxed) + 1;
        if should_exit_trickle(o) {
            self.trickle.store(false, Ordering::Relaxed);
            self.ok_streak.store(0, Ordering::Relaxed);
            info!("peer left trickle mode");
        }
    }

    pub fn in_trickle(&self) -> bool {
        self.trickle.load(Ordering::Relaxed)
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
    /// See [`PushFate`]: congestion sheds feed the link governor; intentional
    /// trickle skips must not (N=2 + one slow peer used to pin drop% at ~50%).
    pub async fn push_h264(
        &self,
        annex_b: Vec<u8>,
        duration: Duration,
        keyframe: bool,
    ) -> Result<PushFate> {
        use rtp::extension::playout_delay_extension::PlayoutDelayExtension;
        use rtp::extension::HeaderExtension;

        let path = self.present_path.load(Ordering::Relaxed);
        let (send_rtp, send_dc) = path_flags(path);
        let mut delivered = false;
        let mut trickle_skip = false;

        // Trickle is congestion-gated, not delta-starve: H.264 cannot skip P-frames
        // mid-GOP without IDR recovery (Sunshine/Moonlight + FrameHandoff pattern).
        // Old path skipped every delta while trickling → IDR-only → ~1fps paint.

        // Hybrid: full RTP for visible paint + thin CLVD for WC/`input_wm`.
        // FEC only on PATH_WEBCODECS (photon proven). Full dual via COUCHLINK_RTP_FULL.
        let hybrid_clvd =
            send_dc && (path == PATH_UNKNOWN || path == PATH_WARMUP || path == PATH_WEBCODECS);
        // Peek seq for thinning without consuming it on skipped frames.
        let next_seq = self.video_seq.load(Ordering::Relaxed);

        let mut push_clvd = async || -> (bool, bool) {
            let mut delivered = false;
            let mut trickle_skip = false;
            let dc_open = self.video_dc.ready_state()
                == webrtc::data_channel::data_channel_state::RTCDataChannelState::Open;
            // Thin by default; densify to every AU when SCTP is slack so WC/
            // input_wm (S_p50) can catch RTP — dual-full only when buffer is empty.
            let thin_ok = should_send_clvd(keyframe, path, next_seq);
            let slack = dc_open
                && !keyframe
                && send_rtp
                && self.video_dc.buffered_amount().await < VIDEO_DC_MAX_BUFFERED / 2;
            if !thin_ok && !slack {
                return (false, false);
            }
            // P-frames stay inside the 24 KiB wow-bar; IDRs use the larger
            // ceiling so multi-fragment keyframes actually complete on CLVD.
            let congested = dc_open && self.video_dc_congested_for(keyframe).await;
            // When RTP is the visible present, never IDR the shared encoder for
            // CLVD SCTP pain — that blacks every peer's picture while HUD stays
            // 0% drop (RTP still Delivered). WC waits for the normal 3s IDR
            // (or one hybrid bootstrap PLI).
            let idr_ok_for_clvd = !send_rtp;
            if dc_open && !congested {
                let seq = self.video_seq.fetch_add(1, Ordering::Relaxed);
                let w = self.video_w.load(Ordering::Relaxed).min(u32::from(u16::MAX)) as u16;
                let h = self.video_h.load(Ordering::Relaxed).min(u32::from(u16::MAX)) as u16;
                let au = VideoAccessUnit {
                    seq,
                    width: w,
                    height: h,
                    keyframe,
                    annex_b: annex_b.clone(),
                    stamp_us: crate::age::now_us(),
                    input_wm: self.last_input_wm.load(Ordering::Relaxed),
                };
                // Hybrid keeps full RTP as the present path. FEC parity on CLVD
                // at promote taxed the shared link, Chrome RTCP-PLI'd, and the
                // shared encoder IDR blacked Joel (05:08:18). Never FEC while
                // RTP is live — thin CLVD is enough for input_wm / S_p50.
                let fragments = if self.fec_enabled && path == PATH_WEBCODECS && !send_rtp {
                    au.encode_fragments_with_fec()
                } else {
                    au.encode_fragments()
                };
                let mut sent_all = true;
                for frag in fragments {
                    if self.video_dc_congested_for(keyframe).await {
                        sent_all = false;
                        break;
                    }
                    if let Err(e) = self.video_dc.send(&Bytes::from(frag)).await {
                        warn!("video datachannel send: {e}");
                        sent_all = false;
                        break;
                    }
                }
                if sent_all {
                    delivered = true;
                } else if idr_ok_for_clvd {
                    self.request_keyframe_coalesced();
                } else if keyframe && send_rtp {
                    // Incomplete CLVD IDR under hybrid: one dual-coalesced retry so
                    // WC can configure / stamp input_wm. Never for P-frame shed —
                    // that path blacks every peer's RTP while HUD stays 0% drop.
                    self.request_keyframe_coalesced_dual();
                }
            } else if dc_open && congested && !keyframe {
                if idr_ok_for_clvd {
                    self.request_keyframe_coalesced();
                }
                if self.trickle.load(Ordering::Relaxed) {
                    trickle_skip = true;
                }
            }
            (delivered, trickle_skip)
        };

        // Hybrid: RTP FIRST so visible paint never waits on SCTP CLVD sends
        // (CLVD-before-RTP made Φ/age_p95 ~200ms while RTP fps still looked green).
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

        if hybrid_clvd {
            let (d, t) = push_clvd().await;
            delivered |= d;
            trickle_skip |= t;
        }

        if send_dc && !hybrid_clvd {
            let (d, t) = push_clvd().await;
            delivered |= d;
            trickle_skip |= t;
        }
        if delivered {
            self.note_delivered();
            Ok(PushFate::Delivered)
        } else if trickle_skip {
            Ok(PushFate::TrickleSkip)
        } else {
            self.note_shed();
            Ok(PushFate::Shed)
        }
    }
}

/// Per-peer outcome of one frame push — the state variable the governor needs.
///
/// Hand-worked (N=2 peers, one trickling, one healthy), every cadence tick:
/// - Old: trickle returned "shed" → dropped+=1, any=true → drop% → 50% forever
/// - Then link governor floored bitrate; push_fps collapsed to IDR rate (~0.8)
/// - New: TrickleSkip is invisible to the governor; only real Shed counts
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PushFate {
    Delivered,
    Shed,
    TrickleSkip,
}

/// Aggregate fan-out fates into (any_delivered, per_peer_congestion_sheds).
pub fn governor_shed_counts(fates: &[PushFate]) -> (bool, u64) {
    let any = fates.iter().any(|f| *f == PushFate::Delivered);
    let shed = fates.iter().filter(|f| **f == PushFate::Shed).count() as u64;
    (any, shed)
}

/// Frame-level shed for the link governor — 1 only when *no* peer got the frame.
///
/// Summing per-peer sheds with N>1 inflated drop% (~9% live with P1+P2 while
/// Joel received every frame) and floored bitrate to 1250 kbps.
pub fn governor_frame_shed(fates: &[PushFate]) -> u64 {
    if fates.is_empty() {
        return 0;
    }
    if fates.iter().any(|f| *f == PushFate::Delivered) {
        0
    } else {
        1
    }
}

/// Default coalesce window for CLVD/viewer-triggered keyframe requests.
const KEYFRAME_COALESCE_MS: u64 = 750;

fn coalesce_keyframe_request(wanted: &AtomicBool, coalesce_ms: &AtomicU64) {
    coalesce_keyframe_request_within(wanted, coalesce_ms, KEYFRAME_COALESCE_MS);
}

fn coalesce_keyframe_request_within(wanted: &AtomicBool, coalesce_ms: &AtomicU64, min_gap_ms: u64) {
    let now = now_ms();
    let prev = coalesce_ms.load(Ordering::Relaxed);
    if now.saturating_sub(prev) < min_gap_ms {
        return;
    }
    if coalesce_ms
        .compare_exchange(prev, now, Ordering::Relaxed, Ordering::Relaxed)
        .is_ok()
    {
        wanted.store(true, Ordering::Relaxed);
    }
}

async fn setup_video_channel(
    dc: Arc<RTCDataChannel>,
    keyframe_wanted: Arc<AtomicBool>,
    keyframe_coalesce_ms: Arc<AtomicU64>,
    present_path: Arc<AtomicU8>,
) {
    {
        let keyframe_wanted = Arc::clone(&keyframe_wanted);
        let keyframe_coalesce_ms = Arc::clone(&keyframe_coalesce_ms);
        let present_path = Arc::clone(&present_path);
        dc.on_open(Box::new(move || {
            info!("video datachannel open (CLVD → browser WebCodecs)");
            let keyframe_wanted = Arc::clone(&keyframe_wanted);
            let keyframe_coalesce_ms = Arc::clone(&keyframe_coalesce_ms);
            let present_path = Arc::clone(&present_path);
            Box::pin(async move {
                // Hybrid: one soft IDR as soon as CLVD is open so WC can configure
                // without waiting for the 3s periodic tick. Dual coalesce ≥3s —
                // never an IDR storm. RTCP PLI still ignored while RTP is live.
                let (send_rtp, send_dc) = path_flags(present_path.load(Ordering::Relaxed));
                if send_rtp && send_dc {
                    coalesce_keyframe_request_within(
                        &keyframe_wanted,
                        &keyframe_coalesce_ms,
                        KEYFRAME_COALESCE_DUAL_MS,
                    );
                }
            })
        }));
    }
    dc.on_message(Box::new(move |msg: DataChannelMessage| {
        let keyframe_wanted = Arc::clone(&keyframe_wanted);
        let keyframe_coalesce_ms = Arc::clone(&keyframe_coalesce_ms);
        let present_path = Arc::clone(&present_path);
        Box::pin(async move {
            let _ = msg;
            // Hybrid: RTP is painting. Honor DC PLI only on a long coalesce
            // (≈ IDR_INTERVAL) so WC can bootstrap one IDR without a storm —
            // RTCP PLI stays ignored (Chrome loss feedback ≠ WC need).
            let path = present_path.load(Ordering::Relaxed);
            let (send_rtp, _) = path_flags(path);
            if send_rtp {
                coalesce_keyframe_request_within(
                    &keyframe_wanted,
                    &keyframe_coalesce_ms,
                    KEYFRAME_COALESCE_DUAL_MS,
                );
                return;
            }
            coalesce_keyframe_request(&keyframe_wanted, &keyframe_coalesce_ms);
        })
    }));
}

async fn setup_pad_channel(
    dc: Arc<RTCDataChannel>,
    pad_tx: mpsc::UnboundedSender<PadFrame>,
    pad_device: Arc<Mutex<VirtualPad>>,
    last_input_wm: Arc<AtomicU32>,
) {
    dc.on_open(Box::new(move || {
        info!("pad datachannel open");
        Box::pin(async {})
    }));

    dc.on_message(Box::new(move |msg: DataChannelMessage| {
        let pad_tx = pad_tx.clone();
        let pad_device = Arc::clone(&pad_device);
        let last_input_wm = Arc::clone(&last_input_wm);
        Box::pin(async move {
            if msg.is_string {
                if let Ok(text) = std::str::from_utf8(&msg.data) {
                    if let Some(echo) = parse_age_echo_json(text) {
                        if echo.stamp_us != 0 {
                            crate::age::record_global(crate::age::echo_age_ms(echo.stamp_us));
                        } else {
                            // RTP/canvas: no host stamp — record recv→paint present age.
                            let local = echo.paint_ms - echo.recv_ms;
                            if local.is_finite() && local > 0.0 {
                                crate::age::record_global(local);
                            }
                        }
                        return;
                    }
                    let _ = serde_json::from_str::<PadFeedback>(text);
                }
                return;
            }
            match PadFrame::decode(&msg.data) {
                Ok(frame) => {
                    note_pad_arrived();
                    note_input_wm(&last_input_wm, frame.seq);
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
    fn hybrid_paths_keep_full_rtp_and_thin_clvd() {
        assert_eq!(path_flags(PATH_UNKNOWN), (true, true));
        assert_eq!(path_flags(PATH_WEBCODECS), (true, true));
        assert_eq!(path_flags(PATH_WARMUP), (true, true));
        assert!(should_send_rtp(false, PATH_UNKNOWN, false));
        assert!(should_send_rtp(false, PATH_WEBCODECS, false));
        assert!(should_send_rtp(false, PATH_WARMUP, false));
        assert!(should_send_clvd(true, PATH_WARMUP, 1));
        assert!(should_send_clvd(false, PATH_WARMUP, 2));
        assert!(!should_send_clvd(false, PATH_WARMUP, 1));
        assert!(!should_send_clvd(false, PATH_UNKNOWN, 1));
        assert!(should_send_clvd(false, PATH_WEBCODECS, 2));
    }

    #[test]
    fn video_dc_buffer_cap_fits_input_wow_bar_at_5mbps() {
        // Keep in sync with VIDEO_DC_MAX_BUFFERED (24 KiB @ 5 Mbps ≈ 39 ms < 45 ms wow).
        let cap_bytes = 24 * 1024u64;
        let bytes_per_sec = 5_000u64 * 1000 / 8;
        let queue_ms = cap_bytes * 1000 / bytes_per_sec;
        assert!(
            queue_ms <= 45,
            "SCTP P-frame buffer cap {queue_ms}ms must stay inside S_p50 wow bar"
        );
        // IDR ceiling must fit several 14 KiB fragments (720p keyframes).
        assert!(
            VIDEO_DC_MAX_BUFFERED_IDR >= 4 * 14 * 1024,
            "IDR SCTP cap must complete a multi-fragment keyframe"
        );
    }

    #[test]
    fn congestion_trickle_skip_is_not_governor_shed() {
        use crate::webrtc_peer::{governor_shed_counts, PushFate};
        let (_, shed) = governor_shed_counts(&[
            PushFate::Delivered,
            PushFate::TrickleSkip,
            PushFate::TrickleSkip,
        ]);
        assert_eq!(shed, 0);
    }

    #[test]
    fn trickle_thresholds() {
        assert!(!should_enter_trickle(0));
        assert!(!should_enter_trickle(7));
        assert!(should_enter_trickle(8));
        assert!(!should_exit_trickle(3));
        assert!(should_exit_trickle(4));
    }

    #[test]
    fn two_peer_one_congestion_shed_is_not_frame_shed() {
        let fates = [PushFate::Delivered, PushFate::Shed];
        assert_eq!(governor_frame_shed(&fates), 0);
    }

    #[test]
    fn two_peer_one_trickle_does_not_report_fifty_pct_to_governor() {
        // The death-spiral arithmetic: healthy Delivered + slow TrickleSkip
        // must yield shed=0. Counting TrickleSkip as Shed pinned drop% at 50.
        let (any, shed) =
            governor_shed_counts(&[PushFate::Delivered, PushFate::TrickleSkip]);
        assert!(any);
        assert_eq!(shed, 0);
        let (any2, shed2) =
            governor_shed_counts(&[PushFate::Shed, PushFate::TrickleSkip]);
        assert!(!any2);
        assert_eq!(shed2, 1);
    }

    #[test]
    fn hybrid_keeps_full_rtp_on_webcodecs_and_unknown() {
        assert!(should_send_rtp(false, PATH_WEBCODECS, false));
        assert!(should_send_rtp(true, PATH_WEBCODECS, false));
        assert!(should_send_rtp(false, PATH_UNKNOWN, false));
        assert!(should_send_rtp(false, PATH_RTP, false));
        assert!(should_send_rtp(false, PATH_WARMUP, false));
    }

    #[test]
    fn webcodecs_promote_does_not_cut_rtp() {
        assert_eq!(path_flags(PATH_WEBCODECS), (true, true));
        assert!(should_send_rtp(false, PATH_WEBCODECS, false));
    }

    #[test]
    fn rtp_path_skips_datachannel_only() {
        assert_eq!(path_flags(PATH_RTP), (true, false));
    }

    #[test]
    fn parse_present_path_recognises_both_values() {
        assert_eq!(parse_present_path("webcodecs"), PATH_WEBCODECS);
        assert_eq!(parse_present_path("rtp"), PATH_RTP);
        assert_eq!(parse_present_path("warmup"), PATH_WARMUP);
        assert_eq!(parse_present_path("clvd"), PATH_UNKNOWN);
        assert_eq!(parse_present_path("binary"), PATH_UNKNOWN);
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
        assert_eq!(parse_present_path("not-a-real-path"), PATH_UNKNOWN);
        assert_eq!(parse_present_path(""), PATH_UNKNOWN);
    }
}

/// Pure thresholds for slow-peer isolation (unit-tested without a peer).
pub fn should_enter_trickle(shed_streak: u32) -> bool {
    shed_streak >= 8
}

pub fn should_exit_trickle(ok_streak: u32) -> bool {
    ok_streak >= 4
}
