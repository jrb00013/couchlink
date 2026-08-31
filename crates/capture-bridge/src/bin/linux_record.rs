//! Dev tool: capture N frames of the live X11 desktop at an interval,
//! convert each through the shared `bgra_to_nv12`, and write raw NV12 to a
//! file for manual inspection (e.g. with ffplay).
//!
//! Usage:
//!   couchlink-capture-bridge-linux --frames 5 --interval-ms 500 --out /tmp/capture.nv12
//!
//! Play back with:
//!   ffplay -f rawvideo -pixel_format nv12 -video_size <W>x<H> /tmp/capture.nv12
//! (W/H are printed to stderr on capture — the desktop's root window size.)
//!
//! Not part of the production couchlink pipeline (there is no Linux host
//! capture path); this exists purely to exercise `couchlink_capture_bridge::color`
//! against real captured pixels, see crates/capture-bridge/src/linux_capture.rs.

#[cfg(not(windows))]
fn main() -> anyhow::Result<()> {
    use anyhow::Context;
    use clap::Parser;
    use couchlink_capture_bridge::color::bgra_to_nv12;
    use couchlink_capture_bridge::linux_capture::capture_root_window;
    use std::io::Write;
    use std::time::Duration;

    #[derive(Parser)]
    struct Args {
        #[arg(long, default_value_t = 5)]
        frames: u32,
        #[arg(long, default_value_t = 200)]
        interval_ms: u64,
        #[arg(long, default_value = "/tmp/couchlink-linux-capture.nv12")]
        out: String,
    }

    let args = Args::parse();
    let mut file = std::fs::File::create(&args.out)
        .with_context(|| format!("create output file {}", args.out))?;

    let mut nv12 = Vec::new();
    for i in 0..args.frames {
        let frame = capture_root_window().context("capture X11 root window")?;
        bgra_to_nv12(&frame.bgra, frame.width, frame.height, &mut nv12);
        file.write_all(&nv12)
            .with_context(|| format!("write frame {i} to {}", args.out))?;
        eprintln!(
            "frame {i}: {}x{} -> {} bytes NV12 (total file: {})",
            frame.width,
            frame.height,
            nv12.len(),
            args.out
        );
        if i + 1 < args.frames {
            std::thread::sleep(Duration::from_millis(args.interval_ms));
        }
    }
    eprintln!("done: wrote {} frame(s) to {}", args.frames, args.out);
    Ok(())
}

#[cfg(windows)]
fn main() {
    eprintln!("couchlink-capture-bridge-linux is a Linux-only dev tool (no-op on Windows).");
}
