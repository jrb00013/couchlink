//! Windows DXGI capture server — streams primary display to couchlink-host in WSL.

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
    use std::net::{SocketAddr, TcpListener, TcpStream};
    use std::time::Duration;
    use tracing::info;

    #[derive(Parser, Debug)]
    #[command(name = "couchlink-win-capture")]
    pub struct Args {
        #[arg(long, default_value = "0.0.0.0:9876")]
        pub bind: String,
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
                    write_frame_sync(&mut writer, w as u32, h as u32, &bgra).context("send frame")?;
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
        let addr: SocketAddr = args.bind.parse().context("bind address")?;
        let listener = TcpListener::bind(addr).context("bind")?;
        info!("Windows capture listening on {addr} (CLFR). WSL: COUCHLINK_WINDOWS_CAPTURE=auto");
        loop {
            let (stream, peer) = listener.accept().context("accept")?;
            info!("client connected from {peer}");
            if let Err(e) = stream_to(stream, args.max_fps) {
                tracing::warn!("client session ended: {e:#}");
            }
        }
    }
}

#[cfg(windows)]
fn main() -> anyhow::Result<()> {
    run::main()
}
