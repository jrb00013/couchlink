mod decode;
mod dualsense_reader;
mod feedback_apply;
mod keyboard_input;
mod xbox_reader;
mod config_file;
mod invite;
mod prompt;
mod reachability;
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
    /// Advertise these IPs as ICE host candidates (WSL: auto-detects Windows LAN IP).
    #[arg(long, env = "COUCHLINK_ICE_IPS", value_delimiter = ',')]
    ice_ips: Vec<String>,
    /// Skip the video window entirely — pad-only, for automation/testing, or
    /// the automatic fallback when window/GPU creation fails.
    #[arg(long, default_value_t = false)]
    headless: bool,
    /// Do not prompt for a join URL (fail if credentials are missing).
    #[arg(long, default_value_t = false)]
    no_prompt: bool,
}

impl Args {
    fn resolve(self) -> Result<ResolvedArgs> {
        let mut args = self;
        let config_url = config_file::read_join_url_from_config();
        let cli_join = args
            .join_url
            .take()
            .or(args.positional_join.take())
            .filter(|s| !s.trim().is_empty());
        let mut join_raw = cli_join
            .clone()
            .or_else(|| std::env::var("COUCHLINK_JOIN_URL").ok().filter(|s| !s.trim().is_empty()))
            .or_else(|| config_url.clone());

        let needs_creds = args.session_id.is_none() || args.pin.is_none();
        // Desktop (windowed): always ask on startup so friends paste a fresh host link
        // (pre-filled from config/env), unless they already passed --join-url on argv.
        // Terminal/headless: ask only when credentials are still missing.
        let should_prompt = !args.no_prompt
            && if args.headless {
                needs_creds && join_raw.is_none()
            } else {
                cli_join.is_none()
            };

        if should_prompt {
            let prefill = join_raw.as_deref().or(config_url.as_deref());
            let prefer_gui = !args.headless;
            let answered = prompt::prompt_join(prefill, prefer_gui)?;
            if let Some(url) = answered.join_url {
                let _ = config_file::write_join_url(&url);
                join_raw = Some(url);
            } else {
                if let Some(s) = answered.session_id {
                    args.session_id = Some(s);
                }
                if let Some(p) = answered.pin {
                    args.pin = Some(p);
                }
                if let Some(sig) = answered.signaling {
                    args.signaling = sig;
                }
                if args.turn_url.is_none() {
                    args.turn_url = answered.turn_url;
                }
                if args.turn_user.is_none() {
                    args.turn_user = answered.turn_user;
                }
                if args.turn_pass.is_none() {
                    args.turn_pass = answered.turn_pass;
                }
            }
        }

        if let Some(url) = join_raw {
            let parsed = invite::parse_join_url(&url)?;
            if args.session_id.is_none() {
                args.session_id = Some(parsed.session_id.clone());
            }
            if args.pin.is_none() {
                args.pin = Some(parsed.pin.clone());
            }
            args.signaling = parsed.signaling.clone();
            if args.turn_url.is_none() {
                args.turn_url = parsed.turn_url.clone();
            }
            if args.turn_user.is_none() {
                args.turn_user = parsed.turn_user.clone();
            }
            if args.turn_pass.is_none() {
                args.turn_pass = parsed.turn_pass.clone();
            }
            if invite::is_headscale_invite(&parsed) {
                info!(
                    "Headscale join link — use ./scripts/join-headscale.sh (or install.sh --online) then route via {}",
                    parsed.signaling
                );
            } else if invite::is_tailscale_invite(&parsed) {
                info!(
                    "Tailscale join link — routing via {} (same tailnet as host)",
                    parsed.signaling
                );
            }
        }

        let ice_ips = reachability::discover_ice_ips(args.ice_ips);

        Ok(ResolvedArgs {
            signaling: args.signaling,
            session_id: args.session_id.context(
                "session id required — paste the join URL when prompted, or pass --join-url / --session-id",
            )?,
            pin: args.pin.context(
                "PIN required — paste the join URL when prompted, or pass --join-url / --pin",
            )?,
            send_pad: args.send_pad,
            turn_url: args.turn_url,
            turn_user: args.turn_user,
            turn_pass: args.turn_pass,
            ice_ips,
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
    ice_ips: Vec<String>,
    headless: bool,
}

fn main() -> Result<()> {
    let cli = Args::parse();
    if cli.headless {
        init_tracing();
        let args = cli.resolve()?;
        return run_headless(args);
    }

    // Windowed: skip the OS dialog — the waiting screen has an editable join field.
    let mut join_prefill = cli
        .join_url
        .clone()
        .or(cli.positional_join.clone())
        .filter(|s| !s.trim().is_empty())
        .or_else(|| std::env::var("COUCHLINK_JOIN_URL").ok().filter(|s| !s.trim().is_empty()))
        .or_else(config_file::read_join_url_from_config)
        .unwrap_or_default();

    loop {
        let resolved = if join_prefill.trim().is_empty() {
            None
        } else {
            match resolve_join_string(&join_prefill, &cli) {
                Ok(a) => Some(a),
                Err(e) => {
                    warn!("join prefill invalid ({e}) — edit the waiting-screen field");
                    None
                }
            }
        };
        match run_windowed(resolved, join_prefill.clone())? {
            view::ViewResult::Closed => return Ok(()),
            view::ViewResult::Rejoin(url) => {
                let _ = config_file::write_join_url(&url);
                join_prefill = url;
            }
        }
    }
}

fn init_tracing() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "couchlink_client=info".into()),
        )
        .try_init()
        .ok();
}

fn resolve_join_string(raw: &str, cli: &Args) -> Result<ResolvedArgs> {
    let parsed = invite::parse_join_input(raw)?;
    if invite::is_headscale_invite(&parsed) {
        info!(
            "Headscale join link — use ./scripts/join-headscale.sh (or install.sh --online) then route via {}",
            parsed.signaling
        );
    } else if invite::is_tailscale_invite(&parsed) {
        info!(
            "Tailscale join link — routing via {} (same tailnet as host)",
            parsed.signaling
        );
    }
    let ice_ips = reachability::discover_ice_ips(cli.ice_ips.clone());
    Ok(ResolvedArgs {
        signaling: parsed.signaling,
        session_id: parsed.session_id,
        pin: parsed.pin,
        send_pad: cli.send_pad,
        turn_url: parsed.turn_url.or_else(|| cli.turn_url.clone()),
        turn_user: parsed.turn_user.or_else(|| cli.turn_user.clone()),
        turn_pass: parsed.turn_pass.or_else(|| cli.turn_pass.clone()),
        ice_ips,
        headless: false,
    })
}

/// Today's exact pad-only behavior, unchanged, just renamed and made callable
/// as a fallback from `run_windowed`. Owns its own Tokio runtime.
fn run_headless(args: ResolvedArgs) -> Result<()> {
    init_tracing();

    let rt = tokio::runtime::Runtime::new()?;
    rt.block_on(async_main(args, None, None))
}


/// Opens the video window on this (main) thread; networking + decode + pad
/// polling run on a background thread with its own Tokio runtime. Falls back
/// to `run_headless` if window/GPU creation fails.
///
/// `join_prefill` seeds the waiting-screen field. `args` starts networking when
/// present; otherwise the window waits until the user submits a join link.
fn run_windowed(args: Option<ResolvedArgs>, join_prefill: String) -> Result<view::ViewResult> {
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

    let net_thread = if let Some(net_args) = args.clone() {
        let net_keyboard_pad = keyboard_pad.clone();
        Some(
            std::thread::Builder::new()
                .name("couchlink-net".into())
                .spawn(move || -> Result<()> {
                    let rt = tokio::runtime::Runtime::new()?;
                    rt.block_on(async_main(
                        net_args,
                        Some(frame_tx),
                        Some((net_keyboard_pad, shutdown_rx)),
                    ))
                })
                .expect("spawn network thread"),
        )
    } else {
        drop(frame_tx);
        drop(shutdown_rx);
        None
    };

    let view_result = match view::run(frame_rx, keyboard_pad, shutdown_tx, join_prefill) {
        Ok(r) => r,
        Err(e) => {
            warn!("windowed viewer failed ({e}), falling back to headless mode");
            if let Some(net_thread) = net_thread {
                if let Err(join_err) = net_thread.join() {
                    warn!("network thread panicked during fallback shutdown: {join_err:?}");
                }
            }
            let Some(args) = args else {
                return Err(e).context("window failed and no join credentials for headless");
            };
            run_headless(args)?;
            return Ok(view::ViewResult::Closed);
        }
    };

    if let Some(net_thread) = net_thread {
        let _ = net_thread.join();
    }
    Ok(view_result)
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

    let turn_ok = args.turn_url.is_some() && args.turn_user.is_some() && args.turn_pass.is_some();
    if reachability::signaling_needs_turn(&args.signaling) && !turn_ok {
        warn!(
            "joining remote host without TURN — WSL/NAT clients often fail ICE. \
             Use the full host join URL (includes turn= / turnu= / turnp=)."
        );
    }
    if reachability::is_wsl() {
        if args.ice_ips.is_empty() {
            info!("WSL client: no Windows LAN IP for ICE host candidates — relying on STUN/TURN");
        } else {
            info!("WSL client: advertising Windows LAN IP(s) for ICE: {:?}", args.ice_ips);
        }
    }

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
        args.ice_ips.clone(),
    )
    .await?;

    let mut dualsense = if args.send_pad {
        dualsense_reader::DualSenseReader::open_first().ok()
    } else {
        None
    };
    let mut xbox = if args.send_pad {
        xbox_reader::XboxReader::open_first().ok()
    } else {
        None
    };
    if dualsense.is_some() {
        info!("DualSense controller connected");
    } else if xbox.is_some() {
        info!("Xbox controller connected");
    } else if args.send_pad {
        if keyboard.is_some() {
            info!("no DualSense/Xbox controller found — keyboard input is still available in windowed mode");
        } else {
            warn!("no DualSense/Xbox controller found and no keyboard available (headless mode) — no pad input will be sent");
        }
    }

    let mut feedback_rx = player.take_feedback_rx().await;
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
                    Some(SignalMessage::Offer { sdp, epoch }) => {
                        info!("got offer");
                        player.handle_offer(sdp, epoch, &signal_out).await?;
                    }
                    Some(SignalMessage::IceCandidate { candidate, sdp_mid, sdp_mline_index }) => {
                        let _ = player.add_ice(candidate, sdp_mid, sdp_mline_index).await;
                    }
                    Some(SignalMessage::StreamInfo { width, height, fps, codec, .. }) => {
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
            fb = async {
                match feedback_rx.as_mut() {
                    Some(rx) => rx.recv().await,
                    None => std::future::pending().await,
                }
            } => {
                match fb {
                    Some(fb) => {
                        if let Err(e) = feedback_apply::apply_feedback(dualsense.as_mut(), &fb) {
                            warn!("apply pad feedback: {e}");
                        }
                    }
                    None => {
                        // All senders dropped (pad DC closed) — stop polling or we busy-loop.
                        feedback_rx = None;
                    }
                }
            }
            _ = pad_interval.tick() => {
                seq = seq.wrapping_add(1);
                // DualSense takes priority for this tick when it produces a frame,
                // then Xbox. Otherwise, only send a keyboard-derived frame while a
                // key is actually held, plus one extra frame on the falling edge
                // (last key just released) so the host doesn't keep a stale input
                // held forever. Without this gate, `read_frame()` returning
                // `Ok(None)` on WouldBlock (the common case at 250Hz) would
                // fall through to a neutral keyboard frame every other tick
                // and stomp whatever the pad was holding.
                let ds_frame = match dualsense.as_mut() {
                    Some(r) => r.read_frame().ok().flatten(),
                    None => None,
                };
                let xb_frame = if ds_frame.is_none() {
                    match xbox.as_mut() {
                        Some(r) => r.read_frame().ok().flatten(),
                        None => None,
                    }
                } else {
                    None
                };
                let frame = match ds_frame.or(xb_frame) {
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
