mod capture;
mod config;
mod encode;
mod invite;
mod motion;
mod scale;
mod signaling_client;
mod webrtc_peer;

use anyhow::Result;
use clap::Parser;
use config::HostArgs;
use couchlink_proto::SignalMessage;
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
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "couchlink_host=info".into()),
        )
        .init();

    let args = HostArgs::parse();
    let preset = args.stream_preset();
    info!(
        "couchlink host session={} preset={}x{}@{} bluetooth_pad={}",
        args.session_id, preset.width, preset.height, preset.fps, args.bluetooth_pad
    );

    // Friend opens this in a browser (same host as signaling static files).
    let public_http = args
        .signaling
        .replacen("ws://", "http://", 1)
        .replacen("wss://", "https://", 1)
        .trim_end_matches("/ws")
        .to_string();
    let turn = match (&args.turn_url, &args.turn_user, &args.turn_pass) {
        (Some(url), Some(user), Some(pass)) => Some(invite::TurnInfo { url, user, pass }),
        _ => None,
    };
    let join = invite::player_invite_url(
        &public_http,
        &args.session_id,
        &args.pin,
        &args.signaling,
        turn,
    );
    info!("friend join URL: {join}");
    if let Ok(qr) = qrcode::QrCode::new(join.as_bytes()) {
        let ste = qr.render::<char>().quiet_zone(false).module_dimensions(2, 1).build();
        eprintln!("\nScan / open join link:\n{ste}\n{join}\n");
    }

    let pad = Arc::new(Mutex::new(webrtc_peer::create_virtual_pad(
        args.bluetooth_pad,
    )?));

    let mut signaling = signaling_client::SignalingClient::connect(&args.signaling).await?;
    signaling
        .register_host(
            args.session_id.clone(),
            args.pin.clone(),
            args.device_name.clone(),
            args.preset.clone(),
            args.emulator.clone(),
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
    let mut attached_player_epoch: u64 = 0;

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
    let frame_dur = Duration::from_millis(1000 / preset.fps.max(1) as u64);
    let idle_dur = Duration::from_millis(1000 / args.idle_fps.max(1) as u64);
    let mut frames_out: u64 = 0;
    let mut rate_window = std::time::Instant::now();
    let mut rate_mark: u64 = 0;
    let mut idle_frames: u64 = 0;
    let (mut stage_capture, mut stage_scale, mut stage_encode, mut stage_push) =
        (Duration::ZERO, Duration::ZERO, Duration::ZERO, Duration::ZERO);
    let mut force_idr = true;
    let mut idr_burst: u32 = 0;
    let mut last_idr = std::time::Instant::now();
    let mut capture_ok_announced: Option<bool> = None;

    // Wait for the first player before offering WebRTC.
    loop {
        let Some(msg) = signaling.inbound.recv().await else {
            return Ok(());
        };
        match msg {
            SignalMessage::PeerJoined { epoch, .. } => {
                info!("player joined — sending offer (player epoch {epoch})");
                attached_player_epoch = epoch;
                host.create_and_send_offer(&signal_out).await?;
                break;
            }
            SignalMessage::Error { message } => warn!("signal error: {message}"),
            _ => {}
        }
    }

    let mut heartbeat = tokio::time::interval(Duration::from_secs(20));
    heartbeat.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

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
                    Some(SignalMessage::Answer { sdp }) => {
                        host.handle_answer(sdp).await?;
                        info!("remote answer set — forcing IDR for browser decoder");
                        force_idr = true;
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
                        attached_player_epoch = epoch;
                        info!("player rejoined (epoch {epoch}) — rebuilding WebRTC peer + offer");
                        let _ = host.pc.close().await;
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
                    }
                    Some(SignalMessage::Heartbeat) => {
                        let _ = signal_out.send(SignalMessage::Pong);
                    }
                    None => break,
                    _ => {}
                }
            }
            _ = tokio::time::sleep(frame_dur) => {
                let t_capture = std::time::Instant::now();
                let Some(bgra) = capturer.capture_bgra()? else { continue };
                let ms_capture = t_capture.elapsed();
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
                    if let Err(e) = host.push_h264(nal, frame_dur).await {
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
