mod capture;
mod config;
mod emulator_pad;
mod encode;
mod invite;
mod latency;
mod link_gov;
mod motion;
mod scale;
mod signaling_client;
mod webrtc_peer;

use anyhow::Result;
use clap::Parser;
use config::HostArgs;
use couchlink_pad::VirtualPad;
use couchlink_proto::SignalMessage;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, AtomicU8};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Mutex;
use tracing::{info, warn};

/// Wall-clock gap between keyframes. Frame-count intervals stretch to many seconds
/// once the encoder throttles on a static screen, stranding late joiners on black.
const IDR_INTERVAL: Duration = Duration::from_secs(2);

/// Longest a single frame push may hold the loop that also drains capture.
///
/// Three frame times at 60fps. Past that the peer is not keeping up and the
/// frame is already too old to be worth showing.
const PUSH_BUDGET: Duration = Duration::from_millis(50);

/// Longest a *keyframe* push may hold the loop.
///
/// A keyframe is the only thing that lets a viewer who joined mid-GOP start
/// painting — the browser's WebCodecs path literally refuses to configure its
/// decoder until an IDR arrives. On a fresh SCTP DataChannel the send is in
/// slow-start, so the very first keyframe is also the most likely to blow the
/// normal 50ms budget. Drop it and the viewer waits for the next scheduled
/// IDR (up to `IDR_INTERVAL` away) — and if the channel is still ramping that
/// one goes too, the browser's fallback timer fires, and the session settles
/// on RTP with its jitter buffer for its entire duration. Keyframes are rare
/// (at most one per `IDR_INTERVAL`), so a generous budget costs nothing in
/// steady state while making the join reliable.
const KEYFRAME_PUSH_BUDGET: Duration = Duration::from_secs(1);

/// Push one frame, but never let it park the caller.
///
/// `push_h264` awaits twice — the SCTP DataChannel and the RTP sample writer —
/// and both apply backpressure. Those awaits live in the same `select!` branch
/// that drains the Windows capture socket, so a peer that stops consuming stalls
/// the whole branch: capture goes unread, its buffer fills, win-capture blocks
/// writing into it, and everything freezes with the host asleep at 0% CPU. The
/// player cannot even rejoin, because the same branch services signaling.
///
/// Guarding the individual awaits is whack-a-mole — the invariant is that
/// nothing here may block indefinitely, so the budget is enforced at the edge
/// and covers any await added inside later.
/// `Ok(true)` means the frame was dropped (budget timeout, or shed by SCTP
/// congestion in `push_h264`), not sent.
///
/// The caller must not count a dropped frame as delivered — this used to
/// return `Ok(())` on both a real send and a timeout indistinguishably, so
/// the periodic fps/stage diagnostics silently over-counted during exactly
/// the congestion they exist to reveal. The same blind spot also shed
/// congestion-stalled frames as "sent", so the link governor never stepped
/// the encoder down on a saturated path.
async fn push_bounded(
    host: &webrtc_peer::WebRtcHost,
    nal: Vec<u8>,
    dur: Duration,
    keyframe: bool,
) -> Result<bool> {
    match tokio::time::timeout(
        if keyframe { KEYFRAME_PUSH_BUDGET } else { PUSH_BUDGET },
        host.push_h264(nal, dur, keyframe),
    )
    .await
    {
        Ok(Ok(shed)) => Ok(shed),
        Ok(Err(e)) => Err(e),
        Err(_) => {
            // Dropped H.264 leaves the decoder referencing frames it never got.
            host.request_keyframe();
            warn!("frame push exceeded budget — dropped, asked for a keyframe");
            Ok(true)
        }
    }
}

/// One remote player's peer connection, virtual controller, and feedback loop.
///
/// The host's own physical pad owns emulator P1, so a `PlayerConn` for slot `s`
/// drives the emulator's P2–P4. Everything here is per-player — only capture,
/// encode, and the fan-out share the single H.264 bitstream.
struct PlayerConn {
    host: Arc<webrtc_peer::WebRtcHost>,
    /// Player epoch this peer was built for; a stale `PeerJoined` for this slot
    /// (epoch older than what is already seated) is ignored.
    attached_player_epoch: u64,
    /// Last controller family this player reported, so the emulator rebind (a
    /// process spawn) only runs when the answer actually changes.
    last_pad_kind: Option<String>,
    /// Rumble/adaptive-trigger feedback loop for this player's pad, aborted on
    /// leave/rebuild so a dead peer stops polling its (dropped) virtual pad.
    pad_feedback_task: Option<tokio::task::JoinHandle<()>>,
}

/// Close a departing/rebuilding player's peer off the critical path.
///
/// Awaiting `pc.close()` inline could hang indefinitely, and this is the same
/// loop that relays video and services signaling — a hang stops everything, for
/// every player, not just the one leaving.
fn close_conn(conn: PlayerConn) {
    if let Some(task) = conn.pad_feedback_task {
        task.abort();
    }
    let old_pc = Arc::clone(&conn.host.pc);
    tokio::spawn(async move {
        if tokio::time::timeout(Duration::from_secs(5), old_pc.close())
            .await
            .is_err()
        {
            warn!("previous peer connection did not close within 5s");
        }
    });
}

/// Build the WebRTC peer + virtual controller for one slot.
///
/// Each slot gets its own `VirtualPad` device (a separate controller on the
/// emulator's P2–P4 port), its own offer epoch, and its own feedback loop —
/// the function signature already supported this; it just never had a caller
/// that used more than one instance.
async fn build_player_conn(
    args: &HostArgs,
    preset: couchlink_proto::StreamPreset,
    signal_out: &tokio::sync::mpsc::UnboundedSender<SignalMessage>,
    slot: u8,
    epoch: u64,
) -> Result<PlayerConn> {
    let offer_epoch = Arc::new(AtomicU64::new(0));
    let player_slot = Arc::new(AtomicU8::new(slot));
    let pad = Arc::new(Mutex::new(webrtc_peer::create_virtual_pad(
        args.bluetooth_pad,
    )?));
    let (host, _pad_rx) = webrtc_peer::WebRtcHost::new(
        signal_out.clone(),
        Arc::clone(&pad),
        args.bluetooth_pad,
        args.turn_url.clone(),
        args.turn_user.clone(),
        args.turn_pass.clone(),
        args.ice_ips.clone(),
        Arc::clone(&offer_epoch),
        Arc::clone(&player_slot),
    )
    .await?;
    host.set_video_size(preset.width, preset.height);
    let host = Arc::new(host);
    let pad_feedback_task = Some(spawn_pad_feedback(Arc::clone(&host), pad));
    Ok(PlayerConn {
        host,
        attached_player_epoch: epoch,
        last_pad_kind: None,
        pad_feedback_task,
    })
}

/// Poll this slot's virtual pad for game HID output (rumble / adaptive
/// triggers from the DualSense VHID companion) and relay it to the player.
///
/// Runs on its own task so a congested feedback send can never park the main
/// loop that also relays video and services signaling.
fn spawn_pad_feedback(
    host: Arc<webrtc_peer::WebRtcHost>,
    pad: Arc<Mutex<VirtualPad>>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(Duration::from_millis(8));
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            tick.tick().await;
            let outs = {
                let mut guard = pad.lock().await;
                guard.poll_feedback().unwrap_or_default()
            };
            for fb in outs {
                if host.send_feedback(&fb).await.is_err() {
                    return;
                }
            }
        }
    })
}

/// Push one frame to every currently-connected slot, concurrently.
///
/// Sequential awaits would be wrong: `push_bounded`'s own budget is up to 50ms
/// per peer (1s for keyframes), and the cadence tick can be as tight as 2ms on
/// the pre-encoded path — four sequential 50ms awaits would stall the whole
/// capture loop. Returns `(received_by_any, dropped_total)`: the caller counts
/// a produced frame once (when at least one viewer took it) and feeds the link
/// governor the *sum* of every slot's sheds — one shared governor commands the
/// one shared encoder knob, so N per-slot governors would fight over it.
async fn push_to_all(
    slots: &Arc<Mutex<HashMap<u8, PlayerConn>>>,
    nal: Vec<u8>,
    dur: Duration,
    keyframe: bool,
) -> (bool, u64) {
    let guard = slots.lock().await;
    if guard.is_empty() {
        return (false, 0);
    }
    let results = futures_util::future::join_all(guard.iter().map(|(_, conn)| {
        push_bounded(&conn.host, nal.clone(), dur, keyframe)
    }))
    .await;
    let mut any = false;
    let mut dropped = 0u64;
    for r in results {
        match r {
            Ok(true) => dropped += 1,
            Ok(false) => any = true,
            Err(e) => warn!("push h264 (fan-out): {e}"),
        }
    }
    (any, dropped)
}

/// Drain a rejoin burst from the inbound queue. One reload (or a double-tab /
/// rapid reload) can enqueue several `PeerJoined`s back-to-back; rebuild once
/// per slot for the newest epoch, and keep every non-join message that arrived
/// during the drain for a deferred replay once the rebuilds are done.
fn take_queued_joins(
    inbound: &mut tokio::sync::mpsc::UnboundedReceiver<SignalMessage>,
    slot: u8,
    epoch: u64,
) -> (Vec<(u8, u64)>, Vec<SignalMessage>) {
    let mut joins: HashMap<u8, u64> = HashMap::new();
    let mut deferred: Vec<SignalMessage> = Vec::new();
    joins.insert(slot, epoch);
    while let Ok(extra) = inbound.try_recv() {
        match extra {
            SignalMessage::PeerJoined { epoch: e, slot: s, .. } => {
                match joins.get(&s) {
                    Some(&existing) if existing >= e => {
                        warn!("dropping older PeerJoined epoch={e} during coalesce (slot {s})");
                    }
                    _ => {
                        joins.insert(s, e);
                    }
                }
            }
            other => deferred.push(other),
        }
    }
    // Deterministic rebuild order (slot 1 first) so logs read the same every run.
    let mut joins: Vec<(u8, u64)> = joins.into_iter().collect();
    joins.sort_by_key(|(s, _)| *s);
    (joins, deferred)
}

/// Route a slot-stamped message (Answer / IceCandidate / RequestOffer) to that
/// slot's peer. Shared by the main signaling branch and the deferred replay
/// after a join burst, so a burst can't interleave with a fresh message.
async fn route_slot_msg(
    msg: SignalMessage,
    slots: &Arc<Mutex<HashMap<u8, PlayerConn>>>,
    signal_out: &tokio::sync::mpsc::UnboundedSender<SignalMessage>,
    force_idr: &mut bool,
) {
    match msg {
        SignalMessage::Answer { sdp, epoch, slot } => {
            if let Some(conn) = slots.lock().await.get(&slot) {
                match conn.host.handle_answer(sdp, epoch).await {
                    Ok(true) => {
                        info!("remote answer set (slot {slot}) — forcing IDR for browser decoder");
                        *force_idr = true;
                    }
                    Ok(false) => {}
                    Err(e) => warn!("answer failed (continuing): {e:#}"),
                }
            } else {
                warn!("answer for unknown slot {slot} dropped");
            }
        }
        SignalMessage::IceCandidate {
            candidate,
            sdp_mid,
            sdp_mline_index,
            slot,
        } => {
            if let Some(conn) = slots.lock().await.get(&slot) {
                let _ = conn.host.add_ice(candidate, sdp_mid, sdp_mline_index).await;
            } else {
                warn!("ice candidate for unknown slot {slot} dropped");
            }
        }
        SignalMessage::RequestOffer { slot } => {
            if let Some(conn) = slots.lock().await.get(&slot) {
                info!("player requested offer (slot {slot}, renegotiate, no peer rebuild)");
                *force_idr = true;
                if let Err(e) = conn.host.create_and_send_offer(signal_out).await {
                    warn!("request_offer failed: {e}");
                }
            } else {
                warn!("request_offer for unknown slot {slot} dropped");
            }
        }
        other => warn!("deferred signal ignored after rejoin coalesce: {other:?}"),
    }
}

/// Build (or rebuild, for a reload) the peer for `slot`, then offer.
///
/// Only this slot's peer and virtual pad are touched — every other player's
/// connection keeps streaming untouched, which is what lets players join and
/// leave without disturbing the ones already seated.
async fn handle_slot_join(
    slot: u8,
    epoch: u64,
    args: &HostArgs,
    preset: couchlink_proto::StreamPreset,
    signal_out: &tokio::sync::mpsc::UnboundedSender<SignalMessage>,
    slots: &Arc<Mutex<HashMap<u8, PlayerConn>>>,
    capturer: &mut capture::FrameCapture,
    capture_ok_announced: Option<bool>,
    force_idr: &mut bool,
) -> Result<()> {
    {
        let guard = slots.lock().await;
        if let Some(existing) = guard.get(&slot) {
            if existing.attached_player_epoch >= epoch {
                warn!(
                    "ignoring stale PeerJoined epoch={epoch} (attached={}) for slot {slot}",
                    existing.attached_player_epoch
                );
                return Ok(());
            }
        }
    }
    info!("player joined slot {slot} (player epoch {epoch}) — building WebRTC peer + offer");
    let conn = build_player_conn(args, preset, signal_out, slot, epoch).await?;
    capturer.resync();
    {
        let mut guard = slots.lock().await;
        if let Some(old) = guard.insert(slot, conn) {
            close_conn(old);
        }
    }
    *force_idr = true;
    let _ = signal_out.send(stream_info_message(
        &preset,
        capture_ok_announced,
        None,
    ));
    // Offer with the lock released: `create_and_send_offer` awaits WebRTC, and
    // nothing else needs the map while it runs.
    let host = slots.lock().await.get(&slot).map(|c| Arc::clone(&c.host));
    if let Some(host) = host {
        if let Err(e) = host.create_and_send_offer(signal_out).await {
            warn!("offer failed for slot {slot}: {e}");
        }
    }
    Ok(())
}

/// One stage's share of a frame's total processing time, for naming the
/// current bottleneck rather than leaving the reader to eyeball four numbers.
fn dominant_stage(stages: &[(&str, Duration)]) -> &'static str {
    stages
        .iter()
        .max_by_key(|(_, d)| *d)
        .map(|(name, _)| match *name {
            "capture" => "capture (Windows→WSL handoff)",
            "scale" => "scale (BGRA resize)",
            "encode" => "encode (H.264)",
            "push" => "push (network send)",
            other => {
                // New stage names must be taught here explicitly rather than
                // silently falling through to a placeholder — a bottleneck
                // label that doesn't say what it means is worse than none.
                debug_assert!(false, "dominant_stage: unlabelled stage {other:?}");
                "unknown"
            }
        })
        .unwrap_or("none")
}


#[tokio::main]
async fn main() -> Result<()> {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let args = HostArgs::parse();
    let verbose = args.verbose
        || matches!(
            std::env::var("COUCHLINK_VERBOSE").as_deref(),
            Ok("1") | Ok("true") | Ok("yes") | Ok("on")
        );
    let default_filter = if verbose {
        "couchlink_host=info,webrtc=info"
    } else {
        "warn,couchlink_host=warn,webrtc=error,webrtc_ice=error,hyper=error,tower_http=error"
    };
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| default_filter.into()),
        )
        .with_target(verbose)
        .init();

    let preset = args.stream_preset();
    if verbose {
        info!(
            "couchlink host session={} preset={}x{}@{} bluetooth_pad={}",
            args.session_id, preset.width, preset.height, preset.fps, args.bluetooth_pad
        );
    }

    // Friend opens this in a browser (same host as signaling static files).
    // Prefer invite_signaling when set so the host can dial 127.0.0.1 while the
    // printed URL still points at the public/WAN address (WSL/NAT hairpin).
    let invite_ws = args
        .invite_signaling
        .as_deref()
        .unwrap_or(args.signaling.as_str());
    let public_http = invite_ws
        .replacen("ws://", "http://", 1)
        .replacen("wss://", "https://", 1)
        .trim_end_matches("/ws")
        .to_string();
    let turn = match (&args.turn_url, &args.turn_user, &args.turn_pass) {
        (Some(url), Some(user), Some(pass)) => Some(invite::TurnInfo { url, user, pass }),
        _ => None,
    };
    let mesh = std::env::var("COUCHLINK_MESH")
        .ok()
        .filter(|s| !s.trim().is_empty());
    let hs_url = std::env::var("COUCHLINK_HS_URL")
        .ok()
        .filter(|s| !s.trim().is_empty());
    let ts_key = std::env::var("COUCHLINK_TS_AUTHKEY")
        .ok()
        .filter(|s| !s.trim().is_empty());
    let headscale = match (&hs_url, &ts_key) {
        (Some(server_url), Some(auth_key)) => Some(invite::HeadscaleInvite {
            server_url,
            auth_key,
        }),
        _ => None,
    };
    // Ship the friend's WireGuard config inside the link when one exists, so a
    // direct tunnel needs no out-of-band file transfer. Opt out with
    // COUCHLINK_INVITE_WG=0 — the config is credential-bearing, and a host that
    // pastes join links into a group chat may not want it embedded.
    let wg_conf = if std::env::var("COUCHLINK_INVITE_WG").as_deref() == Ok("0") {
        None
    } else {
        std::env::var("COUCHLINK_ROOT")
            .ok()
            .map(std::path::PathBuf::from)
            .or_else(|| {
                std::env::current_exe()
                    .ok()
                    .and_then(|e| e.parent()?.parent()?.parent().map(|p| p.to_path_buf()))
            })
            .map(|root| root.join("infra/wireguard/wg0-player.conf"))
            .filter(|p| p.is_file())
            .and_then(|p| std::fs::read_to_string(p).ok())
            .filter(|c| c.contains("[Peer]") && c.contains("Endpoint"))
    };
    let join = invite::player_invite_url(
        &public_http,
        &args.session_id,
        &args.pin,
        invite_ws,
        turn,
        mesh.as_deref(),
        headscale,
        wg_conf.as_deref(),
    );
    // Always surface the invite — this is what the friend needs.
    println!("friend join URL:\n{join}");
    if verbose {
        info!("friend join URL: {join}");
        if mesh.as_deref() == Some("headscale") || (hs_url.is_some() && ts_key.is_some()) {
            info!(
                "Headscale paste-link — friend: ./install.sh --online (no Tailscale Inc account)"
            );
        } else if mesh.as_deref() == Some("tailscale") {
            info!(
                "Tailscale paste-link — friend: ./install.sh --online (paste this URL)"
            );
        }
        if join.contains("://127.") || join.contains("://localhost") {
            info!("join URL is loopback — browser WebCodecs (lowest latency) is available");
        } else if join.starts_with("http://") {
            info!(
                "LAN http join — WebCodecs needs a secure context; prefer http://127.0.0.1:8443/?… \
                 (SSH tunnel / same machine) or https for near-zero latency; RTP fallback still works"
            );
        }
        if let Ok(qr) = qrcode::QrCode::new(join.as_bytes()) {
            let ste = qr.render::<char>().quiet_zone(false).module_dimensions(2, 1).build();
            eprintln!("\nScan / open join link:\n{ste}\n{join}\n");
        }
    } else {
        eprintln!("(QR + detailed logs: pass --verbose / COUCHLINK_VERBOSE=1)");
    }

    // The supervisor owns the socket and re-registers on every reconnect, so a
    // transient signaling failure can no longer orphan the host from its session.
    let mut signaling = signaling_client::SignalingClient::connect_and_register(
        &args.signaling,
        signaling_client::HostRegistration {
            session_id: args.session_id.clone(),
            pin: args.pin.clone(),
            device_name: args.device_name.clone(),
            preset: args.preset.clone(),
            emulator: args.emulator.clone(),
        },
    )
    .await?;

    let signal_out = signaling.outbound.clone();
    // One peer + one virtual controller per remote slot. The host's own pad owns
    // emulator P1; slots 1-3 fill P2-P4.
    let slots: Arc<Mutex<HashMap<u8, PlayerConn>>> = Arc::new(Mutex::new(HashMap::new()));

    // Open capture before the first player so Windows win-capture can connect immediately.
    // Blocking accept is fine here — we are still in startup, before the select loop.
    let windows_spec = effective_windows_capture(&args);
    let mut capturer = capture::FrameCapture::open(windows_spec.as_deref())?;
    info!(
        "capturing {}x{} ({})",
        capturer.width(),
        capturer.height(),
        if windows_spec.is_some() {
            "Windows desktop bridge"
        } else {
            "local display"
        }
    );
    // Command the Windows encoder to match the preset so the wire size, rate and
    // bitrate can never silently diverge from what the host advertises. Without
    // this a directly-launched host and a stale win-capture stream e.g. 1728x1080
    // while the player is told 1280x720 — overloading both the link and a remote
    // decoder that cannot shrink the stream in time.
    capturer.set_target(couchlink_capture_bridge::EncodeTarget {
        width: preset.width,
        height: preset.height,
        fps: preset.fps,
        bitrate_kbps: preset.bitrate_kbps,
    });
    // Close the loop between the link and the Windows encoder: when the push
    // shows persistent sheds, work the commanded target down the rung ladder so
    // the player's decoder stays fed instead of burning keyframe requests on a
    // stream it cannot drain. Only the pre-encoded path feeds it (the local
    // encoder is in-process and already preset-bound).
    let mut link_gov = link_gov::LinkGov::new(couchlink_capture_bridge::EncodeTarget {
        width: preset.width,
        height: preset.height,
        fps: preset.fps,
        bitrate_kbps: preset.bitrate_kbps,
    });
    let mut commanded_target = link_gov.current();
    let mut encoder = encode::H264Encoder::new(preset.width, preset.height, preset.bitrate_kbps)?;
    let mut motion = motion::MotionDetector::new(preset.width, preset.height);
    // Motion is measured on the raw capture, whose size is not the preset size.
    let mut motion_dims: (usize, usize) = (0, 0);
    let mut last_encode = std::time::Instant::now();
    let mut last_push = std::time::Instant::now();
    let idle_dur = Duration::from_millis(1000 / args.idle_fps.max(1) as u64);
    let mut frames_out: u64 = 0;
    let mut rate_window = std::time::Instant::now();
    let mut rate_mark: u64 = 0;
    let mut idle_frames: u64 = 0;
    // Frames the PUSH_BUDGET timeout dropped in the current window — the
    // direct, on-host signal that the peer (or the link to it) can't keep up.
    let mut dropped_frames: u64 = 0;
    let (mut stage_capture, mut stage_scale, mut stage_encode, mut stage_push) =
        (Duration::ZERO, Duration::ZERO, Duration::ZERO, Duration::ZERO);
    let mut force_idr = true;
    let mut idr_burst: u32 = 0;
    let mut last_idr = std::time::Instant::now();
    let mut capture_ok_announced: Option<bool> = None;

    // Wait for the first player before offering WebRTC — but keep draining the
    // capture socket while waiting. With nobody reading it, TCP fills, the Windows
    // side sheds every frame it encodes, and (because a shed frame asks for a
    // keyframe) the encoder degenerates into emitting nothing but IDRs.
    loop {
        let msg = tokio::select! {
            msg = signaling.inbound.recv() => match msg {
                Some(m) => m,
                None => return Ok(()),
            },
            _ = tokio::time::sleep(Duration::from_millis(10)) => {
                // Discard: there is no one to show it to yet.
                let _ = capturer.capture();
                continue;
            }
        };
        match msg {
            SignalMessage::PeerJoined { epoch, slot, .. } => {
                handle_slot_join(
                    slot,
                    epoch,
                    &args,
                    preset,
                    &signal_out,
                    &slots,
                    &mut capturer,
                    capture_ok_announced,
                    &mut force_idr,
                )
                .await?;
                break;
            }
            SignalMessage::Error { message } => warn!("signal error: {message}"),
            _ => {}
        }
    }

    let mut heartbeat = tokio::time::interval(Duration::from_secs(20));
    heartbeat.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

    // The metronome the video is sent on. Delay (not Burst) on a missed tick so a slow
    // frame never causes a catch-up flurry — a burst is exactly the jitter we are
    // trying to remove.
    // When frames arrive pre-encoded, the Windows side owns the cadence and this
    // loop is only a relay — so poll fast and forward immediately. Holding an
    // already-encoded frame for the rest of a 16ms beat is pure added latency.
    // On the raw path this interval *is* the metronome and must stay at frame time.
    let tick = if capturer.is_preencoded() {
        Duration::from_millis(2)
    } else {
        Duration::from_millis(1000 / preset.fps.max(1) as u64)
    };
    let mut cadence = tokio::time::interval(tick);
    cadence.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

    let _ = signal_out.send(stream_info_message(
        &preset,
        None,
        None,
    ));

    loop {
        tokio::select! {
            _ = heartbeat.tick() => {
                let _ = signal_out.send(SignalMessage::Heartbeat);
            }
            msg = signaling.inbound.recv() => {
                match msg {
                    None => break,
                    Some(m) => match m {
                        SignalMessage::Answer { .. }
                        | SignalMessage::IceCandidate { .. }
                        | SignalMessage::RequestOffer { .. } => {
                            route_slot_msg(m, &slots, &signal_out, &mut force_idr).await;
                        }
                        SignalMessage::PeerLeft { slot } => {
                            if slot == 0 {
                                // Pre-slot server broadcast: drop a lone connection,
                                // otherwise don't guess which slot left.
                                let mut guard = slots.lock().await;
                                if guard.len() == 1 {
                                    let only = *guard.keys().next().unwrap();
                                    let old = guard.remove(&only).unwrap();
                                    drop(guard);
                                    info!("player left (slot {only})");
                                    close_conn(old);
                                } else {
                                    let n = guard.len();
                                    drop(guard);
                                    warn!(
                                        "PeerLeft without a slot while {n} players connected — ignoring"
                                    );
                                }
                            } else if let Some(old) = slots.lock().await.remove(&slot) {
                                info!("player left (slot {slot})");
                                close_conn(old);
                            } else {
                                warn!("PeerLeft for unoccupied slot {slot}");
                            }
                        }
                        SignalMessage::PlayersStatus { occupied, max } => {
                            info!("players: {occupied}/{max}");
                        }
                        SignalMessage::PeerJoined { epoch, slot, .. } => {
                            // Coalesce a rejoin burst (double-tab / rapid reload):
                            // rebuild each slot that joined at most once, for the
                            // newest epoch already queued, and replay everything
                            // else that arrived during the drain.
                            let (joins, deferred) =
                                take_queued_joins(&mut signaling.inbound, slot, epoch);
                            for (slot, epoch) in joins {
                                handle_slot_join(
                                    slot,
                                    epoch,
                                    &args,
                                    preset,
                                    &signal_out,
                                    &slots,
                                    &mut capturer,
                                    capture_ok_announced,
                                    &mut force_idr,
                                )
                                .await?;
                            }
                            // Re-handle non-join messages that arrived during the
                            // drain (answers for a *replaced* peer are dropped by
                            // epoch/state checks in the peer itself).
                            for msg in deferred {
                                route_slot_msg(msg, &slots, &signal_out, &mut force_idr).await;
                            }
                        }
                        SignalMessage::PadInfo { kind, id, slot } => {
                            let mut guard = slots.lock().await;
                            let Some(conn) = guard.get_mut(&slot) else {
                                warn!("pad_info for unknown slot {slot} dropped");
                                continue;
                            };
                            if conn.last_pad_kind.as_deref() != Some(kind.as_str()) {
                                conn.last_pad_kind = Some(kind.clone());
                                // Reconcile off the loop: this shells out to the
                                // companion + emulator config, and this same branch
                                // relays video.
                                tokio::task::spawn_blocking(move || {
                                    emulator_pad::apply(&kind, &id, slot)
                                });
                            }
                        }
                        SignalMessage::PresentPath { path, slot } => {
                            if let Some(conn) = slots.lock().await.get(&slot) {
                                conn.host.set_present_path(&path);
                            } else {
                                warn!("present_path for unknown slot {slot} dropped");
                            }
                        }
                        SignalMessage::Heartbeat => {
                            let _ = signal_out.send(SignalMessage::Pong);
                        }
                        _ => {}
                    },
                }
            }
            // Fixed cadence, deliberately NOT driven by frame arrival. WGC hands over
            // frames whenever DWM happens to composite, so arrival-paced sending wobbles
            // between ~20ms and ~60ms gaps. A receiver sizes its jitter buffer from that
            // wobble, and measured buffer grew to ~100ms during motion. Encoding on a
            // metronome makes delivery uniform, which is what lets the buffer stay small.
            _ = cadence.tick() => {
                // Any viewer that lost sync asks for a keyframe over RTCP.
                // Answering immediately turns a multi-second glitch into a single
                // frame. An IDR is decodable by every viewer, so a freshly-joined
                // slot requesting one costs already-connected slots a harmless
                // extra keyframe, not a correctness problem — one shared flag is
                // enough.
                {
                    let guard = slots.lock().await;
                    for (_, conn) in guard.iter() {
                        if conn.host.take_keyframe_request() {
                            force_idr = true;
                        }
                    }
                }
                let t_capture = std::time::Instant::now();
                let Some(frame) = capturer.capture()? else { continue };
                let ms_capture = t_capture.elapsed();

                // Pre-encoded path: Windows already did the expensive work on its GPU,
                // so the host is a pure relay — no scale, no colour conversion, no
                // encode. Everything below this block exists only for raw pixels.
                let bgra = match frame {
                    capture::Captured::H264 { nal, keyframe } => {
                        let (w, h) = (capturer.width() as u32, capturer.height() as u32);
                        {
                            let guard = slots.lock().await;
                            for (_, conn) in guard.iter() {
                                conn.host.set_video_size(w, h);
                            }
                        }
                        // Relay every encoded frame that has arrived, not one per
                        // tick. The encoder's cadence is set on the Windows side; a
                        // backlog here would be shown late, and H.264 frames cannot
                        // be skipped to catch up without corrupting the decoder.
                        let mut queue = vec![(nal, keyframe)];
                        while let Some(capture::Captured::H264 { nal, keyframe }) =
                            capturer.capture()?
                        {
                            queue.push((nal, keyframe));
                            if queue.len() >= 8 {
                                break;
                            }
                        }
                        // Spread the real elapsed time across the burst. Timing each
                        // frame from the previous *push* would report ~1ms for every
                        // frame after the first, so RTP media time would advance far
                        // slower than the wall clock and the receiver would grow its
                        // buffer to cover the drift — delay that accumulates.
                        let burst_gap = last_push
                            .elapsed()
                            .clamp(Duration::from_millis(1), Duration::from_millis(500));
                        let per_frame = burst_gap / queue.len().max(1) as u32;
                        last_push = std::time::Instant::now();
                        for (nal, keyframe) in queue {
                            if keyframe {
                                last_idr = std::time::Instant::now();
                            }
                            let t_push = std::time::Instant::now();
                            let (any, dropped) =
                                push_to_all(&slots, nal, per_frame, keyframe).await;
                            dropped_frames += dropped;
                            if any {
                                frames_out += 1;
                                stage_capture += ms_capture;
                                stage_push += t_push.elapsed();
                                if rate_window.elapsed() >= Duration::from_secs(5) {
                                    let window_frames = frames_out - rate_mark;
                                    let fps =
                                        window_frames as f64 / rate_window.elapsed().as_secs_f64();
                                    let sent = window_frames + dropped_frames;
                                    let drop_pct = if sent > 0 {
                                        dropped_frames * 100 / sent
                                    } else {
                                        0
                                    };
                                    // The pre-encoded encoder is the only component the
                                    // link cannot throttle by itself. If sheds persist,
                                    // step the commanded target down so the player gets
                                    // every frame the link can carry. Drops are summed
                                    // across slots so the single shared governor sees
                                    // the whole vector — N per-slot governors would
                                    // fight over the one encoder knob.
                                    let decided = link_gov.on_window(
                                        dropped_frames as u32,
                                        window_frames as u32,
                                    );
                                    if decided != commanded_target {
                                        commanded_target = decided;
                                        capturer.set_target(decided.clone());
                                        info!(
                                            "link governor: commanded encoder {}x{}@{} ({} kbps) after {}% sheds",
                                            decided.width, decided.height, decided.fps,
                                            decided.bitrate_kbps, drop_pct
                                        );
                                    }
                                    eprintln!(
                                        "[couchlink-host] streaming {fps:.1} fps ({frames_out} frames total, GPU-encoded on Windows) \
                                         | per frame: relay {:.1}ms | dropped {dropped_frames}/{sent} ({drop_pct}%) — {}",
                                        if dropped_frames == 0 {
                                            "link keeping up".to_string()
                                        } else {
                                            format!(
                                                "bottleneck: peer/network can't consume at {:.0} Mbps",
                                                preset.bitrate_kbps as f64 / 1000.0
                                            )
                                        },
                                        (stage_capture / window_frames.max(1) as u32).as_secs_f64()
                                            * 1000.0
                                    );
                                    let _ = signal_out.send(host_stats_message(
                                        fps,
                                        window_frames,
                                        dropped_frames,
                                        drop_pct as u32,
                                        stage_capture,
                                        stage_scale,
                                        stage_encode,
                                        stage_push,
                                        &commanded_target,
                                    ));
                                    rate_window = std::time::Instant::now();
                                    rate_mark = frames_out;
                                    idle_frames = 0;
                                    dropped_frames = 0;
                                    stage_capture = Duration::ZERO;
                                    stage_scale = Duration::ZERO;
                                    stage_encode = Duration::ZERO;
                                    stage_push = Duration::ZERO;
                                }
                            }
                        }
                        // Keyframe control lives on the Windows side here, so ask for
                        // one rather than pretending we own the GOP.
                        if force_idr || idr_burst > 0 {
                            capturer.request_idr();
                            force_idr = false;
                            idr_burst = 0;
                        } else if last_idr.elapsed() >= IDR_INTERVAL {
                            capturer.request_idr();
                            last_idr = std::time::Instant::now();
                        }
                        if capture_ok_announced.is_none() {
                            capture_ok_announced = Some(true);
                            let _ = signal_out.send(stream_info_message(&preset, Some(true), None));
                        }
                        continue;
                    }
                    capture::Captured::Bgra(b) => b,
                };
                let cap_w = capturer.width();
                let cap_h = capturer.height();
                if (cap_w, cap_h) != motion_dims {
                    motion.resize(cap_w as u32, cap_h as u32);
                    motion_dims = (cap_w, cap_h);
                }
                // Detect motion on the raw capture, before any scaling — the scale is
                // the expensive half of the work we are deciding whether to skip.
                let idle = motion.is_idle(&bgra);
                // Static screens skip the encode entirely rather than sleeping. Sleeping
                // is what made input feel laggy: a keystroke landing during an idle sleep
                // waited it out before anything was encoded. Polling stays at frame_dur,
                // so motion is picked up within one frame regardless.
                // On a metronome, a truly static screen still gets a refresh every
                // idle_dur so the cadence never develops holes; only genuinely
                // redundant frames between refreshes are skipped.
                let refresh_due = last_encode.elapsed() >= idle_dur;
                if idle && !force_idr && idr_burst == 0 && !refresh_due && frames_out > 0 {
                    idle_frames += 1;
                    continue;
                }
                last_encode = std::time::Instant::now();
                let t_scale = std::time::Instant::now();
                let scaled = if cap_w as u32 == preset.width
                    && cap_h as u32 == preset.height
                {
                    bgra
                } else {
                    scale::scale_bgra(
                        &bgra,
                        cap_w,
                        cap_h,
                        preset.width as usize,
                        preset.height as usize,
                    )
                };
                if frames_out == 0 {
                    let avg = capture::sample_avg_luma_bgra(&scaled, 4096);
                    let ok = avg >= 8;
                    if !ok {
                        let hint = if windows_spec.is_some() {
                            "Windows capture bridge is connected but frames look black — check the selected window is visible (or re-run with COUCHLINK_CAPTURE_SOURCE=picker)."
                        } else if capture::is_wsl() {
                            "WSL is capturing the Linux desktop (usually black). Restart the host so couchlink-win-capture auto-starts, then pick your game window."
                        } else {
                            "Capture looks black/empty — nothing visible on the host display."
                        };
                        warn!("capture looks black/empty (avg luma ~{avg}/255). {hint}");
                        capture_ok_announced = Some(false);
                        let _ = signal_out.send(stream_info_message(
                            &preset,
                            Some(false),
                            Some(hint.into()),
                        ));
                    } else {
                        info!("capture avg luma ~{avg}/255 (first frames)");
                        capture_ok_announced = Some(true);
                        let _ = signal_out.send(stream_info_message(
                            &preset,
                            Some(true),
                            None,
                        ));
                    }
                }
                // A single IDR can be lost before the browser's decoder is ready, which
                // costs the viewer seconds of black. Send a short burst instead.
                if force_idr {
                    idr_burst = 3;
                    force_idr = false;
                }
                // Periodic IDR so late joiners / stalled decoders can resync. This MUST be
                // time-based: counting encoded frames meant a throttled static screen went
                // 12+ seconds between keyframes, which is exactly how long a reloading
                // browser sat on black waiting for something it could decode.
                if idr_burst > 0 || last_idr.elapsed() >= IDR_INTERVAL {
                    encoder.force_keyframe();
                    idr_burst = idr_burst.saturating_sub(1);
                    last_idr = std::time::Instant::now();
                }
                let ms_scale = t_scale.elapsed();
                let t_encode = std::time::Instant::now();
                let nal = encoder.encode_bgra(&scaled)?;
                let ms_encode = t_encode.elapsed();
                let t_push = std::time::Instant::now();
                if let Some(nal) = nal {
                    // Sample duration must be the REAL gap since the last frame, not the
                    // preset's ideal frame time. Frames arrive whenever Windows renders
                    // one, so claiming a constant 16ms makes RTP media time advance far
                    // slower than wall clock: the receiver falls progressively behind and
                    // grows its jitter buffer to compensate. That is latency that
                    // accumulates the longer you stream.
                    let real_gap = last_push
                        .elapsed()
                        .clamp(Duration::from_millis(1), Duration::from_millis(500));
                    last_push = std::time::Instant::now();
                    let (any, dropped) = push_to_all(
                        &slots,
                        nal.clone(),
                        real_gap,
                        couchlink_proto::annex_b_is_keyframe(&nal),
                    )
                    .await;
                    dropped_frames += dropped;
                    if any {
                        frames_out += 1;
                        stage_capture += ms_capture;
                        stage_scale += ms_scale;
                        stage_encode += ms_encode;
                        stage_push += t_push.elapsed();
                        if rate_window.elapsed() >= Duration::from_secs(5) {
                            let window_frames = frames_out - rate_mark;
                            let fps = window_frames as f64 / rate_window.elapsed().as_secs_f64();
                            // Idle share tells throttling apart from starvation: a low
                            // fps with idle≈100% is the motion detector doing its job,
                            // a low fps with idle≈0% means the pipeline can't keep up.
                            let polled = window_frames + idle_frames;
                            let idle_pct = if polled > 0 {
                                idle_frames * 100 / polled
                            } else {
                                0
                            };
                            let per = window_frames.max(1) as u32;
                            let sent = window_frames + dropped_frames;
                            let drop_pct = if sent > 0 {
                                dropped_frames * 100 / sent
                            } else {
                                0
                            };
                            let stages: [(&str, Duration); 4] = [
                                ("capture", stage_capture / per),
                                ("scale", stage_scale / per),
                                ("encode", stage_encode / per),
                                ("push", stage_push / per),
                            ];
                            eprintln!(
                                "[couchlink-host] streaming {fps:.1} fps ({frames_out} frames total, {idle_pct}% skipped as static) \
                                 | per frame: capture {:.1}ms scale {:.1}ms encode {:.1}ms push {:.1}ms \
                                 | dropped {dropped_frames}/{sent} ({drop_pct}%) | bottleneck: {}",
                                (stage_capture / per).as_secs_f64() * 1000.0,
                                (stage_scale / per).as_secs_f64() * 1000.0,
                                (stage_encode / per).as_secs_f64() * 1000.0,
                                (stage_push / per).as_secs_f64() * 1000.0,
                                dominant_stage(&stages),
                            );
                            let _ = signal_out.send(host_stats_message(
                                fps,
                                window_frames,
                                dropped_frames,
                                drop_pct as u32,
                                stage_capture,
                                stage_scale,
                                stage_encode,
                                stage_push,
                                &couchlink_capture_bridge::EncodeTarget {
                                    width: preset.width,
                                    height: preset.height,
                                    fps: preset.fps,
                                    bitrate_kbps: preset.bitrate_kbps,
                                },
                            ));
                            stage_capture = Duration::ZERO;
                            stage_scale = Duration::ZERO;
                            stage_encode = Duration::ZERO;
                            stage_push = Duration::ZERO;
                            rate_window = std::time::Instant::now();
                            rate_mark = frames_out;
                            idle_frames = 0;
                            dropped_frames = 0;
                        }
                    }
                }
            }
        }
    }

    Ok(())
}

fn effective_windows_capture(args: &HostArgs) -> Option<String> {
    if let Some(ref s) = args.windows_capture {
        if s.is_empty() || s == "0" || s == "false" {
            return None;
        }
        return Some(s.clone());
    }
    if capture::is_wsl() {
        return Some("auto".into());
    }
    None
}

fn stream_info_message(
    preset: &couchlink_proto::StreamPreset,
    capture_ok: Option<bool>,
    capture_hint: Option<String>,
) -> SignalMessage {
    SignalMessage::StreamInfo {
        width: preset.width,
        height: preset.height,
        fps: preset.fps,
        codec: "H264".into(),
        capture_ok,
        capture_hint,
    }
}

/// Per-window host pipeline telemetry for the debug panel. `target_*` is the
/// encoder target currently commanded (the governor's current rung), so the
/// panel can show the host stepping down on a saturated link rather than
/// silently starving.
fn host_stats_message(
    fps: f64,
    frames_out: u64,
    dropped_frames: u64,
    drop_pct: u32,
    capture: Duration,
    scale: Duration,
    encode: Duration,
    push: Duration,
    target: &couchlink_capture_bridge::EncodeTarget,
) -> SignalMessage {
    let per = frames_out.max(1) as u32;
    let avg = |d: Duration| (d / per).as_secs_f64() * 1000.0;
    let stages: [(&str, Duration); 4] = [
        ("capture", capture / per),
        ("scale", scale / per),
        ("encode", encode / per),
        ("push", push / per),
    ];
    SignalMessage::HostStats {
        fps,
        frames_out,
        dropped_frames,
        drop_pct,
        capture_ms: avg(capture),
        scale_ms: avg(scale),
        encode_ms: avg(encode),
        push_ms: avg(push),
        dominant_stage: dominant_stage(&stages).into(),
        target_width: target.width,
        target_height: target.height,
        target_fps: target.fps,
        target_bitrate_kbps: target.bitrate_kbps,
    }
}
