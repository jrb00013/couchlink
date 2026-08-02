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

use anyhow::Result;
use clap::{Parser, ValueEnum};

#[derive(Clone, Copy, Debug, ValueEnum)]
enum BackendKind {
    /// ViGEm DualShock 4 (best for PS emulators today).
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
    #[arg(long, env = "COUCHLINK_DS_VHID_BACKEND", value_enum, default_value_t = BackendKind::Ds4)]
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
    use std::sync::Arc;
    use tracing::{info, warn};

    let hub = session::OutputHub::new();
    let backend = backend::create(args.backend, hub.clone())?;
    info!(
        "DualSense VHID companion ready (backend={:?}). TCP={}:{} pipe={}",
        args.backend,
        args.bind,
        args.port,
        couchlink_pad::vhid_proto::VHID_PIPE_NAME
    );
    info!("Emulator: P1 = host DualSense; P2 = this virtual pad");

    let tcp_backend = Arc::clone(&backend);
    let tcp_hub = hub.clone();
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
        for conn in listener.incoming() {
            match conn {
                Ok(stream) => {
                    let backend = Arc::clone(&tcp_backend);
                    let hub = tcp_hub.clone();
                    std::thread::spawn(move || {
                        if let Err(e) = session::serve_tcp(stream, backend, hub) {
                            warn!("TCP session: {e:#}");
                        }
                    });
                }
                Err(e) => warn!("TCP accept: {e}"),
            }
        }
    });

    let pipe_backend = Arc::clone(&backend);
    pipe_win::serve_pipe(pipe_backend, hub).context("named pipe server")?;
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
