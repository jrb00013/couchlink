mod capture;
mod config;
mod emulator_pad;
mod encode;
mod invite;
mod latency;
mod motion;
mod scale;
mod signaling_client;
mod webrtc_peer;

use anyhow::Result;
use clap::Parser;
use config::HostArgs;
use couchlink_proto::{PadFeedback, SignalMessage};
use std::sync::atomic::AtomicU64;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Mutex;
use tracing::{info, warn};

/// Wall-clock gap between keyframes. Frame-count intervals stretch to many seconds
/// once the encoder throttles on a static screen, stranding late joiners on black.
const IDR_INTERVAL: Duration = Duration::from_secs(2);


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
    let join = invite::player_invite_url(
        &public_http,
        &args.session_id,
        &args.pin,
        invite_ws,
        turn,
        mesh.as_deref(),
        headscale,
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

    let pad = Arc::new(Mutex::new(webrtc_peer::create_virtual_pad(
        args.bluetooth_pad,
    )?));

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
    let offer_epoch = Arc::new(AtomicU64::new(0));
    let mut host = webrtc_peer::WebRtcHost::new(
        signal_out.clone(),
        Arc::clone(&pad),
        args.bluetooth_pad,
        args.turn_url.clone(),
        args.turn_user.clone(),
        args.turn_pass.clone(),
        args.ice_ips.clone(),
        Arc::clone(&offer_epoch),
    )
    .await?
    .0;
    host.set_video_size(preset.width, preset.height);
    let mut attached_player_epoch: u64 = 0;

    // Forward game HID output (from DualSense VHID companion) to the friend's pad.
    let (pad_feedback_tx, mut pad_feedback_rx) = tokio::sync::mpsc::unbounded_channel::<PadFeedback>();
    {
        let pad_poll = Arc::clone(&pad);
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(Duration::from_millis(8));
            tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            loop {
                tick.tick().await;
                let outs = {
                    let mut guard = pad_poll.lock().await;
                    guard.poll_feedback().unwrap_or_default()
                };
                for fb in outs {
                    if pad_feedback_tx.send(fb).is_err() {
                        return;
                    }
                }
            }
        });
    }

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
    let (mut stage_capture, mut stage_scale, mut stage_encode, mut stage_push) =
        (Duration::ZERO, Duration::ZERO, Duration::ZERO, Duration::ZERO);
    let mut force_idr = true;
    let mut idr_burst: u32 = 0;
    // Last controller family reported by the player — reconciling is a process
    // spawn, so only do it when the answer actually changes.
    let mut last_pad_kind: Option<String> = None;
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
            SignalMessage::PeerJoined { epoch, .. } => {
                info!("player joined — sending offer (player epoch {epoch})");
                attached_player_epoch = epoch;
                // Frames have been piling up in the capture socket while nobody was
                // watching. Start from what is on screen now, not from the backlog.
                capturer.resync();
                host.create_and_send_offer(&signal_out).await?;
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
            fb = pad_feedback_rx.recv() => {
                if let Some(fb) = fb {
                    if let Err(e) = host.send_feedback(&fb).await {
                        warn!("pad feedback send: {e}");
                    }
                }
            }
            msg = signaling.inbound.recv() => {
                match msg {
                    Some(SignalMessage::Answer { sdp, epoch }) => {
                        match host.handle_answer(sdp, epoch).await {
                            Ok(true) => {
                                info!("remote answer set — forcing IDR for browser decoder");
                                force_idr = true;
                            }
                            Ok(false) => {}
                            Err(e) => warn!("answer failed (continuing): {e:#}"),
                        }
                    }
                    Some(SignalMessage::IceCandidate { candidate, sdp_mid, sdp_mline_index }) => {
                        let _ = host.add_ice(candidate, sdp_mid, sdp_mline_index).await;
                    }
                    Some(SignalMessage::PeerLeft) => {
                        warn!("player left");
                    }
                    Some(SignalMessage::RequestOffer) => {
                        info!("player requested offer (renegotiate, no peer rebuild)");
                        force_idr = true;
                        if let Err(e) = host.create_and_send_offer(&signal_out).await {
                            warn!("request_offer failed: {e}");
                        }
                    }
                    Some(SignalMessage::PeerJoined { epoch, .. }) => {
                        if epoch < attached_player_epoch {
                            warn!("ignoring stale PeerJoined epoch={epoch} (attached={attached_player_epoch})");
                            continue;
                        }
                        // Coalesce a rejoin burst (double-tab / rapid reload): only
                        // rebuild once for the newest epoch already queued.
                        let mut epoch = epoch;
                        let mut deferred: Vec<SignalMessage> = Vec::new();
                        while let Ok(extra) = signaling.inbound.try_recv() {
                            match extra {
                                SignalMessage::PeerJoined { epoch: e, .. } => {
                                    if e >= epoch {
                                        epoch = e;
                                    } else {
                                        warn!("dropping older PeerJoined epoch={e} during coalesce");
                                    }
                                }
                                other => deferred.push(other),
                            }
                        }
                        attached_player_epoch = epoch;
                        info!("player rejoined (epoch {epoch}) — rebuilding WebRTC peer + offer");
                        capturer.resync();
                        // Close the previous peer off the critical path.
                        //
                        // Awaiting close() here could hang indefinitely, and because
                        // this is the same select! loop that relays video and services
                        // signaling, a hang stopped *everything*: no offer for this
                        // player, no frames, and no reaction to anyone joining later.
                        // The first player never hit it (no peer to close yet), so it
                        // looked like "only one player can ever connect".
                        let old_pc = Arc::clone(&host.pc);
                        tokio::spawn(async move {
                            if tokio::time::timeout(Duration::from_secs(5), old_pc.close())
                                .await
                                .is_err()
                            {
                                warn!("previous peer connection did not close within 5s");
                            }
                        });
                        host = webrtc_peer::WebRtcHost::new(
                            signal_out.clone(),
                            Arc::clone(&pad),
                            args.bluetooth_pad,
                            args.turn_url.clone(),
                            args.turn_user.clone(),
                            args.turn_pass.clone(),
                            args.ice_ips.clone(),
                            Arc::clone(&offer_epoch),
                        )
                        .await?
                        .0;
                        if let Err(e) = host.create_and_send_offer(&signal_out).await {
                            warn!("offer on rejoin failed: {e}");
                        }
                        force_idr = true;
                        let _ = signal_out.send(stream_info_message(
                            &preset,
                            capture_ok_announced,
                            None,
                        ));
                        // Re-handle non-join messages that arrived during coalesce
                        // (answers for the *old* peer are dropped by epoch/state checks).
                        for msg in deferred {
                            match msg {
                                SignalMessage::Answer { sdp, epoch } => {
                                    match host.handle_answer(sdp, epoch).await {
                                        Ok(true) => {
                                            info!("remote answer set — forcing IDR for browser decoder");
                                            force_idr = true;
                                        }
                                        Ok(false) => {}
                                        Err(e) => warn!("answer failed (continuing): {e:#}"),
                                    }
                                }
                                SignalMessage::IceCandidate {
                                    candidate,
                                    sdp_mid,
                                    sdp_mline_index,
                                } => {
                                    let _ = host.add_ice(candidate, sdp_mid, sdp_mline_index).await;
                                }
                                SignalMessage::RequestOffer => {
                                    force_idr = true;
                                    if let Err(e) = host.create_and_send_offer(&signal_out).await {
                                        warn!("request_offer failed: {e}");
                                    }
                                }
                                other => {
                                    warn!("deferred signal ignored after rejoin coalesce: {other:?}");
                                }
                            }
                        }
                    }
                    Some(SignalMessage::PadInfo { kind, id }) => {
                        // Reconcile off the loop: this shells out to the
                        // companion + emulator config, and this same branch
                        // relays video.
                        if last_pad_kind.as_deref() != Some(kind.as_str()) {
                            last_pad_kind = Some(kind.clone());
                            tokio::task::spawn_blocking(move || {
                                emulator_pad::apply(&kind, &id)
                            });
                        }
                    }
                    Some(SignalMessage::Heartbeat) => {
                        let _ = signal_out.send(SignalMessage::Pong);
                    }
                    None => break,
                    _ => {}
                }
            }
            // Fixed cadence, deliberately NOT driven by frame arrival. WGC hands over
            // frames whenever DWM happens to composite, so arrival-paced sending wobbles
            // between ~20ms and ~60ms gaps. A receiver sizes its jitter buffer from that
            // wobble, and measured buffer grew to ~100ms during motion. Encoding on a
            // metronome makes delivery uniform, which is what lets the buffer stay small.
            _ = cadence.tick() => {
                // A viewer that lost sync asks for a keyframe over RTCP. Answering
                // immediately turns a multi-second glitch into a single frame.
                if host.take_keyframe_request() {
                    force_idr = true;
                }
                let t_capture = std::time::Instant::now();
                let Some(frame) = capturer.capture()? else { continue };
                let ms_capture = t_capture.elapsed();

                // Pre-encoded path: Windows already did the expensive work on its GPU,
                // so the host is a pure relay — no scale, no colour conversion, no
                // encode. Everything below this block exists only for raw pixels.
                let bgra = match frame {
                    capture::Captured::H264 { nal, keyframe } => {
                        host.set_video_size(
                            capturer.width() as u32,
                            capturer.height() as u32,
                        );
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
                        if let Err(e) = host.push_h264(nal, per_frame, keyframe).await {
                            warn!("push h264: {e}");
                        } else {
                            frames_out += 1;
                            stage_capture += ms_capture;
                            if rate_window.elapsed() >= Duration::from_secs(5) {
                                let window_frames = frames_out - rate_mark;
                                let fps =
                                    window_frames as f64 / rate_window.elapsed().as_secs_f64();
                                info!(
                                    "streaming {fps:.1} fps ({frames_out} frames total, GPU-encoded on Windows) \
                                     | per frame: relay {:.1}ms",
                                    (stage_capture / window_frames.max(1) as u32).as_secs_f64()
                                        * 1000.0
                                );
                                rate_window = std::time::Instant::now();
                                rate_mark = frames_out;
                                idle_frames = 0;
                                stage_capture = Duration::ZERO;
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
                    if let Err(e) = host.push_h264(
                        nal.clone(),
                        real_gap,
                        couchlink_proto::annex_b_is_keyframe(&nal),
                    )
                    .await {
                        warn!("push h264: {e}");
                    } else {
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
                            info!(
                                "streaming {fps:.1} fps ({frames_out} frames total, {idle_pct}% skipped as static)                                  | per frame: capture {:.1}ms scale {:.1}ms encode {:.1}ms push {:.1}ms",
                                (stage_capture / per).as_secs_f64() * 1000.0,
                                (stage_scale / per).as_secs_f64() * 1000.0,
                                (stage_encode / per).as_secs_f64() * 1000.0,
                                (stage_push / per).as_secs_f64() * 1000.0,
                            );
                            stage_capture = Duration::ZERO;
                            stage_scale = Duration::ZERO;
                            stage_encode = Duration::ZERO;
                            stage_push = Duration::ZERO;
                            rate_window = std::time::Instant::now();
                            rate_mark = frames_out;
                            idle_frames = 0;
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
