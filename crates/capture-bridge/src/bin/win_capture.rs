//! Windows capture client — streams a monitor or window to couchlink-host in WSL.
//! Connects outbound to the WSL listener (localhost forwarding by default).

#[cfg(not(windows))]
fn main() {
    eprintln!("couchlink-win-capture must be built and run on Windows.");
    std::process::exit(1);
}

#[cfg(windows)]
mod run {
    use anyhow::{bail, Context as AnyhowContext, Result};
    use clap::{Parser, ValueEnum};
    use couchlink_capture_bridge::write_frame_sync;
    use std::io::BufWriter;
    use std::net::TcpStream;
    use std::sync::mpsc;
    use std::time::{Duration, Instant};
    use tracing::{info, warn};
    use windows_capture::capture::{Context, GraphicsCaptureApiHandler};
    use windows_capture::frame::Frame;
    use windows_capture::graphics_capture_api::InternalCaptureControl;
    use windows_capture::graphics_capture_picker::GraphicsCapturePicker;
    use windows_capture::monitor::Monitor;
    use windows_capture::settings::{
        ColorFormat, CursorCaptureSettings, DirtyRegionSettings, DrawBorderSettings,
        MinimumUpdateIntervalSettings, SecondaryWindowSettings, Settings,
    };
    use windows_capture::window::Window;

    #[derive(Clone, Copy, Debug, ValueEnum)]
    pub enum CaptureSource {
        Desktop,
        Picker,
        Window,
    }

    #[derive(Parser, Debug)]
    #[command(name = "couchlink-win-capture")]
    pub struct Args {
        #[arg(long, default_value = "127.0.0.1:9876")]
        pub connect: String,
        #[arg(long, default_value = "60")]
        pub max_fps: u32,
        #[arg(long, value_enum, default_value_t = CaptureSource::Picker)]
        pub source: CaptureSource,
        #[arg(long, default_value = "")]
        pub window: String,
        #[arg(long, default_value_t = false)]
        pub list_windows: bool,
    }

    type FrameMsg = (u32, u32, Vec<u8>);
    type CaptureError = Box<dyn std::error::Error + Send + Sync>;

    struct BridgeCapture {
        tx: mpsc::SyncSender<FrameMsg>,
        frame_dur: Duration,
        last: Instant,
        scratch: Vec<u8>,
    }

    impl GraphicsCaptureApiHandler for BridgeCapture {
        type Flags = (mpsc::SyncSender<FrameMsg>, Duration);
        type Error = CaptureError;

        fn new(ctx: Context<Self::Flags>) -> Result<Self, Self::Error> {
            Ok(Self {
                tx: ctx.flags.0,
                frame_dur: ctx.flags.1,
                last: Instant::now() - Duration::from_secs(1),
                scratch: Vec::new(),
            })
        }

        fn on_frame_arrived(
            &mut self,
            frame: &mut Frame,
            _capture_control: InternalCaptureControl,
        ) -> Result<(), Self::Error> {
            if self.last.elapsed() < self.frame_dur {
                return Ok(());
            }
            self.last = Instant::now();
            let buffer = frame.buffer()?;
            let w = buffer.width();
            let h = buffer.height();
            let pixels = buffer.as_nopadding_buffer(&mut self.scratch).to_vec();
            let _ = self.tx.try_send((w, h, pixels));
            Ok(())
        }

        fn on_closed(&mut self) -> Result<(), Self::Error> {
            warn!("capture source closed");
            Ok(())
        }
    }

    fn list_windows() -> Result<()> {
        let wins = Window::enumerate().context("enumerate windows")?;
        for w in wins {
            let title = w.title().unwrap_or_default();
            if title.trim().is_empty() {
                continue;
            }
            let proc = w.process_name().unwrap_or_else(|_| "?".into());
            println!("{proc}\t{title}");
        }
        Ok(())
    }

    fn spawn_tcp_writer(connect: String, rx: mpsc::Receiver<FrameMsg>) {
        std::thread::spawn(move || loop {
            match TcpStream::connect(&connect) {
                Ok(stream) => {
                    let _ = stream.set_nodelay(true);
                    info!("connected to {connect}");
                    let mut writer = BufWriter::new(stream);
                    while let Ok((w, h, bgra)) = rx.recv() {
                        if let Err(e) = write_frame_sync(&mut writer, w, h, &bgra) {
                            warn!("send frame failed: {e:#} — reconnecting");
                            break;
                        }
                    }
                }
                Err(e) => {
                    tracing::debug!("connect {connect}: {e}");
                    std::thread::sleep(Duration::from_millis(750));
                }
            }
        });
    }

    pub fn main() -> Result<()> {
        tracing_subscriber::fmt()
            .with_env_filter(
                tracing_subscriber::EnvFilter::try_from_default_env()
                    .unwrap_or_else(|_| "couchlink_win_capture=info".into()),
            )
            .init();

        let args = Args::parse();
        if args.list_windows {
            return list_windows();
        }

        let (tx, rx) = mpsc::sync_channel::<FrameMsg>(2);
        spawn_tcp_writer(args.connect.clone(), rx);

        let frame_dur = Duration::from_millis(1000 / args.max_fps.max(1) as u64);
        let flags = (tx, frame_dur);

        // Capture::start owns this thread (required: picker item is !Send).
        match args.source {
            CaptureSource::Desktop => {
                let m = Monitor::primary().context("primary monitor")?;
                info!("capturing primary monitor → {}", args.connect);
                let settings = Settings::new(
                    m,
                    CursorCaptureSettings::WithCursor,
                    DrawBorderSettings::Default,
                    SecondaryWindowSettings::Default,
                    MinimumUpdateIntervalSettings::Custom(frame_dur),
                    DirtyRegionSettings::Default,
                    ColorFormat::Bgra8,
                    flags,
                );
                BridgeCapture::start(settings).map_err(|e| anyhow::anyhow!("{e}"))?;
            }
            CaptureSource::Picker => {
                info!("open the Windows capture picker and choose a window or monitor…");
                let item = GraphicsCapturePicker::pick_item().context("capture picker")?;
                let Some(item) = item else {
                    bail!("no capture target selected");
                };
                info!("picker selection accepted → {}", args.connect);
                let settings = Settings::new(
                    item,
                    CursorCaptureSettings::WithCursor,
                    DrawBorderSettings::Default,
                    SecondaryWindowSettings::Default,
                    MinimumUpdateIntervalSettings::Custom(frame_dur),
                    DirtyRegionSettings::Default,
                    ColorFormat::Bgra8,
                    flags,
                );
                BridgeCapture::start(settings).map_err(|e| anyhow::anyhow!("{e}"))?;
            }
            CaptureSource::Window => {
                if args.window.trim().is_empty() {
                    bail!("--source window requires --window TITLE_SUBSTRING");
                }
                let w = Window::from_contains_name(&args.window)
                    .with_context(|| format!("no window matching '{}'", args.window))?;
                info!(
                    "capturing window '{}' → {}",
                    w.title().unwrap_or_else(|_| args.window.clone()),
                    args.connect
                );
                let settings = Settings::new(
                    w,
                    CursorCaptureSettings::WithCursor,
                    DrawBorderSettings::Default,
                    SecondaryWindowSettings::Default,
                    MinimumUpdateIntervalSettings::Custom(frame_dur),
                    DirtyRegionSettings::Default,
                    ColorFormat::Bgra8,
                    flags,
                );
                BridgeCapture::start(settings).map_err(|e| anyhow::anyhow!("{e}"))?;
            }
        }
        Ok(())
    }
}

#[cfg(windows)]
fn main() -> anyhow::Result<()> {
    run::main()
}
