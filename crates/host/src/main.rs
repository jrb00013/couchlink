mod capture;
mod config;
mod encode;
mod motion;
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
    let (host, mut _pad_rx) =
        webrtc_peer::WebRtcHost::new(signal_out.clone(), Arc::clone(&pad), args.bluetooth_pad)
            .await?;

    // Wait for player, then offer
    loop {
        let Some(msg) = signaling.inbound.recv().await else {
            break;
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
    let mut encoder = encode::H264Encoder::new(preset.width, preset.height, preset.bitrate_kbps)?;
    let mut motion = motion::MotionDetector::new(preset.width, preset.height);
    let frame_dur = Duration::from_millis(1000 / preset.fps.max(1) as u64);
    let idle_dur = Duration::from_millis(1000 / args.idle_fps.max(1) as u64);

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
                        info!("remote answer set");
                    }
                    Some(SignalMessage::IceCandidate { candidate, sdp_mid, sdp_mline_index }) => {
                        let _ = host.add_ice(candidate, sdp_mid, sdp_mline_index).await;
                    }
                    Some(SignalMessage::PeerLeft) => {
                        warn!("player left");
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
                // Note: production path should scale capturer buffer to preset size.
                let idle = motion.is_idle(&bgra);
                if idle {
                    tokio::time::sleep(idle_dur.saturating_sub(frame_dur / 4)).await;
                }
                if let Some(nal) = encoder.encode_bgra(&bgra)? {
                    if let Err(e) = host.push_h264(nal, frame_dur).await {
                        warn!("push h264: {e}");
                    }
                }
            }
        }
    }

    Ok(())
}
