mod dualsense_reader;
mod signaling_client;
mod webrtc_player;

use anyhow::Result;
use clap::Parser;
use couchlink_proto::SignalMessage;
use tracing::{info, warn};

#[derive(Parser, Debug)]
#[command(name = "couchlink-client", about = "Join a couchlink co-play session", version)]
struct Args {
    #[arg(long, env = "COUCHLINK_SIGNALING", default_value = "ws://127.0.0.1:8443/ws")]
    signaling: String,
    #[arg(long, env = "COUCHLINK_SESSION_ID")]
    session_id: String,
    #[arg(long, env = "COUCHLINK_PIN")]
    pin: String,
    /// Poll DualSense and send pad frames even without video decode UI.
    #[arg(long, default_value_t = true)]
    send_pad: bool,
}

#[tokio::main]
async fn main() -> Result<()> {
    let _ = rustls::crypto::ring::default_provider().install_default();
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "couchlink_client=info".into()),
        )
        .init();

    let args = Args::parse();
    let mut signaling = signaling_client::SignalingClient::connect(&args.signaling).await?;
    signaling
        .register_player(args.session_id.clone(), args.pin.clone())
        .await?;

    let signal_out = signaling.outbound.clone();
    let player = webrtc_player::WebRtcPlayer::new(signal_out.clone()).await?;

    let mut reader = if args.send_pad {
        Some(dualsense_reader::DualSenseReader::open_first()?)
    } else {
        None
    };

    let mut pad_interval = tokio::time::interval(std::time::Duration::from_millis(4)); // ~250 Hz

    loop {
        tokio::select! {
            msg = signaling.inbound.recv() => {
                match msg {
                    Some(SignalMessage::Offer { sdp }) => {
                        info!("got offer");
                        player.handle_offer(sdp, &signal_out).await?;
                    }
                    Some(SignalMessage::IceCandidate { candidate, sdp_mid, sdp_mline_index }) => {
                        let _ = player.add_ice(candidate, sdp_mid, sdp_mline_index).await;
                    }
                    Some(SignalMessage::StreamInfo { width, height, fps, codec }) => {
                        info!("stream {width}x{height}@{fps} {codec}");
                    }
                    Some(SignalMessage::PeerLeft) => warn!("host left"),
                    None => break,
                    _ => {}
                }
            }
            _ = pad_interval.tick() => {
                if let Some(r) = reader.as_mut() {
                    if let Some(frame) = r.read_frame()? {
                        if let Err(e) = player.send_pad(&frame).await {
                            warn!("send pad: {e}");
                        }
                    }
                }
            }
        }
    }
    Ok(())
}
