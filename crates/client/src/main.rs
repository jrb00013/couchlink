mod decode;
mod dualsense_reader;
mod feedback_apply;
mod keyboard_input;
mod config_file;
mod invite;
mod signaling_client;
mod view;
mod webrtc_player;

use anyhow::{Context, Result};
use clap::Parser;
use couchlink_proto::SignalMessage;
use keyboard_input::KeyboardPad;
use std::sync::{mpsc as std_mpsc, Arc, Mutex};
use tracing::{info, warn};

#[derive(Parser, Debug, Clone)]
#[command(name = "couchlink-client", about = "Join a couchlink co-play session", version)]
struct Args {
    /// Full invite link from the host (same URL as the browser join page).
    #[arg(long)]
    join_url: Option<String>,
    /// Invite link passed as the only argument (desktop shortcuts / AppImage %u).
    #[arg(value_name = "JOIN_URL")]
    positional_join: Option<String>,
    #[arg(long, env = "COUCHLINK_SIGNALING", default_value = "ws://127.0.0.1:8443/ws")]
    signaling: String,
    #[arg(long, env = "COUCHLINK_SESSION_ID")]
    session_id: Option<String>,
    #[arg(long, env = "COUCHLINK_PIN")]
    pin: Option<String>,
    /// Poll DualSense and send pad frames even without video decode UI.
    #[arg(long, default_value_t = true)]
    send_pad: bool,
    #[arg(long, env = "COUCHLINK_TURN_URL")]
    turn_url: Option<String>,
    #[arg(long, env = "COUCHLINK_TURN_USER")]
    turn_user: Option<String>,
    #[arg(long, env = "COUCHLINK_TURN_PASS")]
    turn_pass: Option<String>,
    /// Skip the video window entirely — pad-only, for automation/testing, or
    /// the automatic fallback when window/GPU creation fails.
    #[arg(long, default_value_t = false)]
    headless: bool,
}

impl Args {
    fn resolve(self) -> Result<ResolvedArgs> {
        let mut args = self;
        let join_raw = args
            .join_url
            .take()
            .or(args.positional_join.take())
            .or_else(config_file::read_join_url_from_config);

        if let Some(url) = join_raw {
            let parsed = invite::parse_join_url(&url)?;
            if args.session_id.is_none() {
                args.session_id = Some(parsed.session_id);
            }
            if args.pin.is_none() {
                args.pin = Some(parsed.pin);
            }
            args.signaling = parsed.signaling;
            if args.turn_url.is_none() {
                args.turn_url = parsed.turn_url;
            }
            if args.turn_user.is_none() {
                args.turn_user = parsed.turn_user;
            }
            if args.turn_pass.is_none() {
                args.turn_pass = parsed.turn_pass;
            }
        }

        Ok(ResolvedArgs {
            signaling: args.signaling,
            session_id: args
                .session_id
                .context("session id required — use --join-url, set join_url= in config, or pass --session-id")?,
            pin: args
                .pin
                .context("PIN required — use --join-url, set join_url= in config, or pass --pin")?,
            send_pad: args.send_pad,
            turn_url: args.turn_url,
            turn_user: args.turn_user,
            turn_pass: args.turn_pass,
            headless: args.headless,
        })
    }
}

#[derive(Debug, Clone)]
struct ResolvedArgs {
    signaling: String,
    session_id: String,
    pin: String,
    send_pad: bool,
    turn_url: Option<String>,
    turn_user: Option<String>,
    turn_pass: Option<String>,
    headless: bool,
}

fn main() -> Result<()> {
    let args = Args::parse().resolve()?;

    if args.headless {
        run_headless(args)
    } else {
        run_windowed(args)
    }
}

/// Today's exact pad-only behavior, unchanged, just renamed and made callable
/// as a fallback from `run_windowed`. Owns its own Tokio runtime.
fn run_headless(args: ResolvedArgs) -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "couchlink_client=info".into()),
        )
        .try_init()
        .ok();

    let rt = tokio::runtime::Runtime::new()?;
    rt.block_on(async_main(args, None, None))
}

/// Opens the video window on this (main) thread; networking + decode + pad
/// polling run on a background thread with its own Tokio runtime. Falls back
/// to `run_headless` if window/GPU creation fails.
fn run_windowed(args: ResolvedArgs) -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "couchlink_client=info".into()),
        )
        .try_init()
        .ok();

    let (frame_tx, frame_rx) = std_mpsc::channel::<decode::DecodedFrame>();
    let (shutdown_tx, shutdown_rx) = std_mpsc::channel::<()>();
    let keyboard_pad = Arc::new(Mutex::new(KeyboardPad::new()));

    let net_args = args.clone();
    let net_keyboard_pad = keyboard_pad.clone();
    let net_thread = std::thread::Builder::new()
        .name("couchlink-net".into())
        .spawn(move || -> Result<()> {
            let rt = tokio::runtime::Runtime::new()?;
            rt.block_on(async_main(
                net_args,
                Some(frame_tx),
                Some((net_keyboard_pad, shutdown_rx)),
            ))
        })
        .expect("spawn network thread");

    match view::run(frame_rx, keyboard_pad, shutdown_tx) {
        Ok(()) => {}
        Err(e) => {
            warn!("windowed viewer failed ({e}), falling back to headless mode");
            // `shutdown_tx` was moved into `view::run` and is dropped now that
            // it has returned; the net thread observes that as a disconnect
            // (see the shutdown check in `async_main`) and winds down on its
            // own. Join it before starting a second networking stack so the
            // two never run concurrently against the same session.
            if let Err(join_err) = net_thread.join() {
                warn!("network thread panicked during fallback shutdown: {join_err:?}");
            }
            return run_headless(args);
        }
    }

    let _ = net_thread.join();
    Ok(())
}

/// Shared networking core for both modes. In windowed mode, `video_frame_out`
/// forwards decoded frames to the window thread and `keyboard` supplies the
/// keyboard-derived pad state plus a shutdown signal from the window.
async fn async_main(
    args: ResolvedArgs,
    video_frame_out: Option<std_mpsc::Sender<decode::DecodedFrame>>,
    keyboard: Option<(Arc<Mutex<KeyboardPad>>, std_mpsc::Receiver<()>)>,
) -> Result<()> {
    let _ = rustls::crypto::ring::default_provider().install_default();

    let mut signaling = signaling_client::SignalingClient::connect(&args.signaling).await?;
    signaling
        .register_player(args.session_id.clone(), args.pin.clone())
        .await?;

    let signal_out = signaling.outbound.clone();
    let (player, mut video_frames) = webrtc_player::WebRtcPlayer::new(
        signal_out.clone(),
        args.turn_url.clone(),
        args.turn_user.clone(),
        args.turn_pass.clone(),
    )
    .await?;

    let mut dualsense = if args.send_pad {
        dualsense_reader::DualSenseReader::open_first().ok()
    } else {
        None
    };
    if dualsense.is_none() && args.send_pad {
        if keyboard.is_some() {
            info!("no DualSense found — keyboard input is still available in windowed mode");
        } else {
            warn!("no DualSense found and no keyboard available (headless mode) — no pad input will be sent");
        }
    }

    let mut pad_interval = tokio::time::interval(std::time::Duration::from_millis(4)); // ~250 Hz
    let mut seq: u32 = 0;
    let mut keyboard_was_active = false;

    loop {
        // Non-blocking check for the window's shutdown signal (windowed mode only).
        if let Some((_, shutdown_rx)) = &keyboard {
            // Treat either an explicit shutdown message or the sender being
            // dropped (disconnected) as "stop now" — otherwise a dropped
            // `shutdown_tx` (e.g. windowed viewer returning early) leaves
            // this loop running forever in the background.
            if !matches!(shutdown_rx.try_recv(), Err(std::sync::mpsc::TryRecvError::Empty)) {
                info!("window closed, shutting down network thread");
                break;
            }
        }

        tokio::select! {
            msg = signaling.inbound.recv() => {
                match msg {
                    Some(SignalMessage::Offer { sdp, epoch: _ }) => {
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
            frame = video_frames.recv() => {
                if let (Some(frame), Some(out)) = (frame, &video_frame_out) {
                    let _ = out.send(frame);
                }
            }
            _ = pad_interval.tick() => {
                seq = seq.wrapping_add(1);
                // DualSense takes priority for this tick when it produces a frame.
                // Otherwise, only send a keyboard-derived frame while a key is
                // actually held, plus one extra frame on the falling edge (last
                // key just released) so the host doesn't keep a stale input
                // held forever. Without this gate, `read_frame()` returning
                // `Ok(None)` on WouldBlock (the common case at 250Hz) would
                // fall through to a neutral keyboard frame every other tick
                // and stomp whatever the DualSense was holding.
                let ds_frame = match dualsense.as_mut() {
                    Some(r) => r.read_frame().ok().flatten(),
                    None => None,
                };
                let frame = match ds_frame {
                    Some(f) => Some(f),
                    None => keyboard.as_ref().and_then(|(kp, _)| {
                        let kp = kp.lock().unwrap();
                        let active = kp.any_key_active();
                        let send = active || keyboard_was_active;
                        keyboard_was_active = active;
                        send.then(|| kp.to_pad_frame(seq))
                    }),
                };
                if let Some(frame) = frame {
                    if let Err(e) = player.send_pad(&frame).await {
                        warn!("send pad: {e}");
                    }
                }
            }
        }
    }
    Ok(())
}
