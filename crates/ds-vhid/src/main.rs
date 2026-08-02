//! DualSense VHID companion for Windows.
//!
//! Listens on TCP `127.0.0.1:39251` so both native Windows and WSL couchlink-host
//! processes can inject friend pad state into a virtual controller for
//! RPCS3/PCSX2 (player 2).
//!
//! Backend today: ViGEm DualShock 4 (requires ViGEmBus). True `054c:0ce6`
//! DualSense via WinUHid can plug in later behind the same DSVH/DSVO protocol.
//!
//! Host physical DualSense remains player 1 — this companion only creates P2.

use anyhow::Result;
use clap::Parser;
use tracing::info;

#[derive(Parser, Debug)]
#[command(
    name = "couchlink-ds-vhid",
    about = "DualSense VHID companion for couchlink (Windows)"
)]
struct Args {
    /// TCP port for WSL / remote host connections (default 39251).
    #[arg(long, env = "COUCHLINK_DS_VHID_PORT", default_value_t = couchlink_pad::vhid_proto::VHID_TCP_PORT)]
    port: u16,
    /// Bind address for TCP (default 127.0.0.1).
    #[arg(long, default_value = "127.0.0.1")]
    bind: String,
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
    use couchlink_pad::vhid_proto::{decode_input, DSVH_MAGIC};
    use std::io::{Read, Write};
    use std::net::{TcpListener, TcpStream};
    use std::sync::{Arc, Mutex};
    use tracing::warn;

    let backend = Arc::new(Mutex::new(VigemBackend::create()?));
    info!(
        "DualSense VHID companion ready (ViGEm DS4 backend). TCP={}:{}",
        args.bind, args.port
    );
    info!("Bind RPCS3/PCSX2 player 2 to the ViGEm DualShock 4 — host physical DualSense stays P1");

    let bind = format!("{}:{}", args.bind, args.port);
    let listener = TcpListener::bind(&bind).with_context(|| format!("bind {bind}"))?;
    info!("listening TCP {bind} (native Windows + WSL host path)");
    for conn in listener.incoming() {
        match conn {
            Ok(stream) => {
                let backend = Arc::clone(&backend);
                std::thread::spawn(move || {
                    if let Err(e) = handle_tcp_client(stream, backend) {
                        warn!("TCP client: {e:#}");
                    }
                });
            }
            Err(e) => warn!("TCP accept: {e}"),
        }
    }
    Ok(())
}

#[cfg(windows)]
struct VigemBackend {
    target: vigem_client::DualShock4Wired<vigem_client::Client>,
}

#[cfg(windows)]
impl VigemBackend {
    fn create() -> Result<Self> {
        use anyhow::Context;
        let client = vigem_client::Client::connect()
            .context("ViGEmBus connect — install https://github.com/nefarius/ViGEmBus/releases")?;
        let id = vigem_client::TargetId::DUALSHOCK4_WIRED;
        let mut target = vigem_client::DualShock4Wired::new(client, id);
        target.plugin().context("ViGEm DS4 plugin")?;
        target.wait_ready().context("ViGEm DS4 wait_ready")?;
        Ok(Self { target })
    }

    fn apply_ds_report(&mut self, report: &[u8; 64]) -> Result<()> {
        use anyhow::Context;
        let lx = report[1];
        let ly = report[2];
        let rx = report[3];
        let ry = report[4];
        let l2 = report[5];
        let r2 = report[6];
        let bl = report[8];
        let bh = report[9];
        let be = report[10];
        let mut btn = (bl & 0x0F) as u16;
        if btn > 8 {
            btn = 8;
        }
        btn |= (bl & 0xF0) as u16;
        btn |= (bh as u16) << 8;
        let special = be & 0x03;
        let ds4 = vigem_client::DS4Report {
            thumb_lx: lx,
            thumb_ly: ly,
            thumb_rx: rx,
            thumb_ry: ry,
            buttons: btn,
            special,
            trigger_l: l2,
            trigger_r: r2,
        };
        self.target.update(&ds4).context("ViGEm DS4 update")?;
        Ok(())
    }
}

#[cfg(windows)]
fn handle_tcp_client(
    stream: std::net::TcpStream,
    backend: std::sync::Arc<std::sync::Mutex<VigemBackend>>,
) -> Result<()> {
    use anyhow::Context;
    use couchlink_pad::vhid_proto::{decode_input, DSVH_MAGIC};
    use std::io::Read;
    use tracing::{info, warn};

    stream.set_nodelay(true)?;
    info!("TCP client connected from {}", stream.peer_addr()?);
    let mut stream = stream;
    let mut buf = vec![0u8; 4 + 1 + 64];
    loop {
        if let Err(e) = stream.read_exact(&mut buf) {
            warn!("client disconnected: {e}");
            break;
        }
        if &buf[0..4] != DSVH_MAGIC {
            warn!("bad DSVH magic — dropping connection");
            break;
        }
        let report = decode_input(&buf)?;
        backend
            .lock()
            .unwrap()
            .apply_ds_report(&report)
            .context("apply report")?;
    }
    Ok(())
}
