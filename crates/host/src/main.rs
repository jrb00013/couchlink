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
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Mutex;
use tracing::{info, warn};

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
    let mut host = webrtc_peer::WebRtcHost::new(
        signal_out.clone(),
        Arc::clone(&pad),
        args.bluetooth_pad,
        args.turn_url.clone(),
        args.turn_user.clone(),
        args.turn_pass.clone(),
        args.ice_ips.clone(),
    )
    .await?
    .0;

    // Wait for the first player before opening the capture/encode loop.
    loop {
        let Some(msg) = signaling.inbound.recv().await else {
            return Ok(());
        };
        match msg {
            SignalMessage::PeerJoined { .. } => {
                info!("player joined — sending offer");
                host.create_and_send_offer(&signal_out).await?;
                break;
            }
            SignalMessage::Error { message } => warn!("signal error: {message}"),
            _ => {}
        }
    }

    let mut capturer = capture::FrameCapture::primary()?;
    info!(
        "capturing primary display {}x{}",
        capturer.width, capturer.height
    );
    let mut encoder = encode::H264Encoder::new(preset.width, preset.height, preset.bitrate_kbps)?;
    let mut motion = motion::MotionDetector::new(preset.width, preset.height);
    let frame_dur = Duration::from_millis(1000 / preset.fps.max(1) as u64);
    let idle_dur = Duration::from_millis(1000 / args.idle_fps.max(1) as u64);
    let mut frames_out: u64 = 0;
    let mut force_idr = true;

    let _ = signal_out.send(SignalMessage::StreamInfo {
        width: preset.width,
        height: preset.height,
        fps: preset.fps,
        codec: "H264".into(),
    });

    loop {
        tokio::select! {
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
                    Some(SignalMessage::PeerJoined { .. }) => {
                        // Fresh browser peer = fresh RTCPeerConnection. Reusing the
                        // old PC after a refresh/leave sends a renegotiation offer the
                        // new player can't answer as an initial offer → join/leave loop.
                        info!("player rejoined — rebuilding WebRTC peer + offer");
                        let _ = host.pc.close().await;
                        host = webrtc_peer::WebRtcHost::new(
                            signal_out.clone(),
                            Arc::clone(&pad),
                            args.bluetooth_pad,
                            args.turn_url.clone(),
                            args.turn_user.clone(),
                            args.turn_pass.clone(),
                            args.ice_ips.clone(),
                        )
                        .await?
                        .0;
                        if let Err(e) = host.create_and_send_offer(&signal_out).await {
                            warn!("offer on rejoin failed: {e}");
                        }
                        force_idr = true;
                        let _ = signal_out.send(SignalMessage::StreamInfo {
                            width: preset.width,
                            height: preset.height,
                            fps: preset.fps,
                            codec: "H264".into(),
                        });
                    }
                    Some(SignalMessage::Heartbeat) => {
                        let _ = signal_out.send(SignalMessage::Pong);
                    }
                    None => break,
                    _ => {}
                }
            }
            _ = tokio::time::sleep(frame_dur) => {
                let Some(bgra) = capturer.capture_bgra()? else { continue };
                let scaled = if capturer.width as u32 == preset.width
                    && capturer.height as u32 == preset.height
                {
                    bgra
                } else {
                    scale::scale_bgra(
                        &bgra,
                        capturer.width,
                        capturer.height,
                        preset.width as usize,
                        preset.height as usize,
                    )
                };
                let idle = motion.is_idle(&scaled);
                if idle {
                    tokio::time::sleep(idle_dur.saturating_sub(frame_dur / 4)).await;
                }
                // Periodic IDR so late joiners / stalled decoders can resync (~2s @ 30fps).
                if force_idr || frames_out % 60 == 0 {
                    encoder.force_keyframe();
                    force_idr = false;
                }
                if let Some(nal) = encoder.encode_bgra(&scaled)? {
                    if let Err(e) = host.push_h264(nal, frame_dur).await {
                        warn!("push h264: {e}");
                    } else {
                        frames_out += 1;
                        if frames_out == 1 || frames_out % 120 == 0 {
                            info!("encoded {frames_out} H264 frames so far");
                        }
                    }
                }
            }
        }
    }

    Ok(())
}
