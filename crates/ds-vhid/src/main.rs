//! DualSense VHID companion for Windows.
//!
//! Serves TCP `:39251` and named pipe `\\.\pipe\couchlink-ds-vhid` so native
//! Windows and WSL hosts can drive Player 2. Host physical DualSense = P1.

#[cfg(windows)]
mod backend;
#[cfg(windows)]
mod pipe_win;
#[cfg(windows)]
mod session;
#[cfg(windows)]
mod winuhid;

use anyhow::Result;
use clap::{Parser, ValueEnum};

#[derive(Clone, Copy, Debug, ValueEnum)]
enum BackendKind {
    /// Prefer WinUHid DualSense (`054c:0ce6`), else ViGEm DualShock 4.
    Auto,
    /// True DualSense via WinUHidDevs.dll (requires WinUHid driver MSI).
    WinUhid,
    /// ViGEm DualShock 4 (good for PS emulators; limited output capture).
    Ds4,
    /// ViGEm Xbox 360 with rumble notifications → DSVO feedback to friend.
    Xbox360,
}

#[derive(Parser, Debug)]
#[command(
    name = "couchlink-ds-vhid",
    about = "DualSense VHID companion for couchlink (Windows)"
)]
struct Args {
    #[arg(long, env = "COUCHLINK_DS_VHID_PORT", default_value_t = couchlink_pad::vhid_proto::VHID_TCP_PORT)]
    port: u16,
    #[arg(long, default_value = "127.0.0.1")]
    bind: String,
    #[arg(long, env = "COUCHLINK_DS_VHID_BACKEND", value_enum, default_value_t = BackendKind::Auto)]
    backend: BackendKind,
}

fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info".into()),
        )
        .init();

    let args = Args::parse();

    #[cfg(not(windows))]
    {
        let _ = args;
        anyhow::bail!("couchlink-ds-vhid is a Windows companion — build/run on Windows");
    }

    #[cfg(windows)]
    {
        run_windows(args)
    }
}

#[cfg(windows)]
fn run_windows(args: Args) -> Result<()> {
    use anyhow::Context;
    use std::net::TcpListener;
    use tracing::{info, warn};

    info!(
        "DualSense VHID companion ready (backend={:?}). TCP={}:{} pipe={}",
        args.backend,
        args.bind,
        args.port,
        couchlink_pad::vhid_proto::VHID_PIPE_NAME
    );
    info!("Emulator: P1 = host's own pad; each connection here plugs in one more (P2, P3, P4…)");

    let tcp_backend_kind = args.backend;
    let bind = format!("{}:{}", args.bind, args.port);
    std::thread::spawn(move || {
        let listener = match TcpListener::bind(&bind) {
            Ok(l) => l,
            Err(e) => {
                warn!("TCP bind {bind} failed: {e}");
                return;
            }
        };
        info!("listening TCP {bind}");
        let mut next_player: u32 = 1;
        for conn in listener.incoming() {
            match conn {
                Ok(stream) => {
                    let player = next_player;
                    next_player += 1;
                    // Each connection is one player's pad. A fresh backend means a
                    // fresh ViGEm target — ViGEmBus assigns the next free XInput/DS4
                    // slot on plugin() — and a fresh hub means this player only ever
                    // hears its own rumble feedback, never another player's.
                    let hub = session::OutputHub::new();
                    let backend = match backend::create(tcp_backend_kind, hub.clone()) {
                        Ok(b) => b,
                        Err(e) => {
                            warn!("player {player}: virtual pad create failed: {e:#}");
                            continue;
                        }
                    };
                    std::thread::spawn(move || {
                        if let Err(e) = session::serve_tcp(stream, backend, hub) {
                            warn!("TCP session (player {player}): {e:#}");
                        }
                    });
                }
                Err(e) => warn!("TCP accept: {e}"),
            }
        }
    });

    pipe_win::serve_pipe(args.backend).context("named pipe server")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    #[test]
    fn proto_dsvo_roundtrip_for_rumble_path() {
        use couchlink_pad::feedback::build_usb_output_report;
        use couchlink_pad::vhid_proto::{decode_output, encode_output};
        use couchlink_proto::PadFeedback;
        let fb = PadFeedback::Rumble {
            large: 200,
            small: 40,
        };
        let report = build_usb_output_report(&fb);
        let enc = encode_output(&report);
        let back = decode_output(&enc).unwrap();
        assert_eq!(back[0], 0x02);
        assert_eq!(back[3], 40);
        assert_eq!(back[4], 200);
    }
}
