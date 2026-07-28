//! Windows DXGI capture client — streams primary display to couchlink-host in WSL.
//! Connects outbound to WSL (localhost forwarding) so Windows Firewall inbound is not required.

#[cfg(not(windows))]
fn main() {
    eprintln!("couchlink-win-capture must be built and run on Windows (DXGI capture).");
    std::process::exit(1);
}

#[cfg(windows)]
mod run {
    use anyhow::{Context, Result};
    use clap::Parser;
    use couchlink_capture_bridge::write_frame_sync;
    use scrap::{Capturer, Display};
    use std::io::BufWriter;
    use std::net::TcpStream;
    use std::time::Duration;
    use tracing::info;

    #[derive(Parser, Debug)]
    #[command(name = "couchlink-win-capture")]
    pub struct Args {
        /// WSL host listener (Windows localhost forwards to WSL by default).
        #[arg(long, default_value = "127.0.0.1:9876")]
        pub connect: String,
        #[arg(long, default_value = "60")]
        pub max_fps: u32,
    }

    fn pack_frame(frame: &[u8], width: usize, height: usize) -> Vec<u8> {
        let row = width * 4;
        let stride = if height > 0 { frame.len() / height } else { row };
        if stride == row {
            return frame.to_vec();
        }
        let mut tight = vec![0u8; row * height];
        for y in 0..height {
            tight[y * row..(y + 1) * row].copy_from_slice(&frame[y * stride..y * stride + row]);
        }
        tight
    }

    fn stream_to(mut stream: TcpStream, max_fps: u32) -> Result<()> {
        stream.set_nodelay(true).ok();
        let display = Display::primary().context("primary display")?;
        let w = display.width();
        let h = display.height();
        info!("capturing Windows display {w}x{h}");
        let mut capturer = Capturer::new(display).context("capturer")?;
        let frame_dur = Duration::from_millis(1000 / max_fps.max(1) as u64);
        let mut writer = BufWriter::new(&mut stream);
        loop {
            let start = std::time::Instant::now();
            match capturer.frame() {
                Ok(raw) => {
                    let bgra = pack_frame(&raw, w, h);
                    write_frame_sync(&mut writer, w as u32, h as u32, &bgra)
                        .context("send frame")?;
                }
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {}
                Err(e) => return Err(e.into()),
            }
            let elapsed = start.elapsed();
            if elapsed < frame_dur {
                std::thread::sleep(frame_dur - elapsed);
            }
        }
    }

    pub fn main() -> Result<()> {
        tracing_subscriber::fmt()
            .with_env_filter(
                tracing_subscriber::EnvFilter::try_from_default_env()
                    .unwrap_or_else(|_| "couchlink_win_capture=info".into()),
            )
            .init();
        let args = Args::parse();
        info!(
            "Windows capture connecting to {} (WSL listener)",
            args.connect
        );
        loop {
            match TcpStream::connect(&args.connect) {
                Ok(stream) => {
                    info!("connected to {}", args.connect);
                    if let Err(e) = stream_to(stream, args.max_fps) {
                        tracing::warn!("session ended: {e:#} — reconnecting…");
                    }
                }
                Err(e) => {
                    tracing::debug!("connect {}: {e} — retry", args.connect);
                }
            }
            std::thread::sleep(Duration::from_millis(750));
        }
    }
}

#[cfg(windows)]
fn main() -> anyhow::Result<()> {
    run::main()
}
