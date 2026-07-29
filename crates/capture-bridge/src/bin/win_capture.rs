//! Windows capture client — streams a monitor or window to couchlink-host in WSL.
//! Connects outbound to the WSL listener (use the WSL eth0 IP, not 127.0.0.1).

#[cfg(not(windows))]
fn main() {
    eprintln!("couchlink-win-capture must be built and run on Windows.");
    std::process::exit(1);
}

#[cfg(windows)]
mod run {
    use anyhow::{bail, Context as AnyhowContext, Result};
    use clap::{Parser, ValueEnum};
    use couchlink_capture_bridge::mf_encoder::HardwareEncoder;
    use couchlink_capture_bridge::{write_frame_with_format, FrameFormat};
    use std::sync::atomic::{AtomicBool, Ordering};
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
        /// WSL host listener — use the WSL IP (e.g. 172.18.x.x:9876), not 127.0.0.1.
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
        /// Downscale to fit this box before sending. Frames cross a WSL virtual NIC
        /// uncompressed, so wire bytes — not the encoder — set the frame rate:
        /// 1080p BGRA is 7.9MB/frame, about 64MB/s, i.e. ~8fps. Sending at the
        /// stream's actual resolution is the single biggest win available.
        #[arg(long, default_value_t = 1280)]
        pub max_width: u32,
        #[arg(long, default_value_t = 720)]
        pub max_height: u32,
        /// Keep a minimized window rendering by parking it off-screen instead
        /// (`--source window` only — DWM stops compositing true minimized windows).
        #[arg(long, default_value_t = true, action = clap::ArgAction::Set)]
        pub keep_rendering: bool,
        /// Encode H.264 on the GPU here instead of shipping raw pixels for the host
        /// to encode. Falls back automatically if no hardware encoder is present.
        #[arg(long, default_value_t = true, action = clap::ArgAction::Set)]
        pub gpu_encode: bool,
        #[arg(long, default_value_t = 8000)]
        pub bitrate_kbps: u32,
    }

    /// width, height, payload, format, keyframe
    type FrameMsg = (u32, u32, Vec<u8>, FrameFormat, bool);

    /// Set by the socket reader when the host asks for an IDR (a player joined and
    /// needs something it can decode from scratch). Read by the capture thread,
    /// which owns the encoder.
    static IDR_REQUESTED: AtomicBool = AtomicBool::new(false);
    type CaptureError = Box<dyn std::error::Error + Send + Sync>;

    struct BridgeCapture {
        tx: mpsc::SyncSender<FrameMsg>,
        frame_dur: Duration,
        last: Instant,
        scratch: Vec<u8>,
        max_w: u32,
        max_h: u32,
        /// Built lazily: COM objects are not Send, so the encoder must be created on
        /// this thread, and its size is only known once a frame has been captured.
        encoder: Option<HardwareEncoder>,
        enc_cfg: EncoderConfig,
        /// Latched after a failure so we do not retry COM setup every frame.
        enc_failed: bool,
        arrived: u32,
        sent: u32,
        dropped: u32,
        rate_window: Instant,
    }

    #[derive(Clone, Copy)]
    pub struct EncoderConfig {
        pub enabled: bool,
        pub fps: u32,
        pub bitrate_bps: u32,
    }

    /// Nearest-neighbour box fit, preserving aspect. Cheap enough to run on the
    /// capture thread and it removes multiples of the wire cost downstream.
    fn downscale_bgra(src: &[u8], sw: u32, sh: u32, dw: u32, dh: u32) -> Vec<u8> {
        let mut out = vec![0u8; (dw * dh * 4) as usize];
        for y in 0..dh {
            let sy = (y * sh / dh).min(sh - 1) as usize;
            for x in 0..dw {
                let sx = (x * sw / dw).min(sw - 1) as usize;
                let si = (sy * sw as usize + sx) * 4;
                let di = ((y * dw + x) * 4) as usize;
                if let Some(px) = src.get(si..si + 4) {
                    out[di..di + 4].copy_from_slice(px);
                }
            }
        }
        out
    }

    /// Target size that fits (sw, sh) inside the box without upscaling. Rounded down
    /// to even in both axes: H.264 chroma is subsampled 2x2, so odd dimensions are
    /// rejected outright by hardware encoders.
    fn fit(sw: u32, sh: u32, max_w: u32, max_h: u32) -> (u32, u32) {
        if sw == 0 || sh == 0 {
            return (sw, sh);
        }
        let (w, h) = if sw <= max_w && sh <= max_h {
            (sw, sh)
        } else if sw * max_h <= max_w * sh {
            ((sw * max_h / sh).max(1), max_h)
        } else {
            (max_w, (sh * max_w / sw).max(1))
        };
        ((w & !1).max(2), (h & !1).max(2))
    }

    impl GraphicsCaptureApiHandler for BridgeCapture {
        type Flags = (mpsc::SyncSender<FrameMsg>, Duration, u32, u32, EncoderConfig);
        type Error = CaptureError;

        fn new(ctx: Context<Self::Flags>) -> Result<Self, Self::Error> {
            Ok(Self {
                tx: ctx.flags.0,
                frame_dur: ctx.flags.1,
                last: Instant::now() - Duration::from_secs(1),
                scratch: Vec::new(),
                max_w: ctx.flags.2,
                max_h: ctx.flags.3,
                encoder: None,
                enc_cfg: ctx.flags.4,
                enc_failed: false,
                arrived: 0,
                sent: 0,
                dropped: 0,
                rate_window: Instant::now(),
            })
        }

        fn on_frame_arrived(
            &mut self,
            frame: &mut Frame,
            _capture_control: InternalCaptureControl,
        ) -> Result<(), Self::Error> {
            self.arrived += 1;
            if self.rate_window.elapsed() >= Duration::from_secs(5) {
                let secs = self.rate_window.elapsed().as_secs_f64();
                // Tells apart "Windows isn't rendering the window" from "the wire is
                // full": arrived is what WGC gave us, sent is what reached the socket.
                info!(
                    "capture {:.1} fps arrived, {:.1} fps sent, {} dropped (queue full)",
                    self.arrived as f64 / secs,
                    self.sent as f64 / secs,
                    self.dropped
                );
                self.arrived = 0;
                self.sent = 0;
                self.dropped = 0;
                self.rate_window = Instant::now();
            }
            if self.last.elapsed() < self.frame_dur {
                return Ok(());
            }
            self.last = Instant::now();
            let buffer = frame.buffer()?;
            let w = buffer.width();
            let h = buffer.height();
            let raw = buffer.as_nopadding_buffer(&mut self.scratch);
            let (dw, dh) = fit(w, h, self.max_w, self.max_h);
            let (w, h, pixels) = if (dw, dh) == (w, h) {
                (w, h, raw.to_vec())
            } else {
                (dw, dh, downscale_bgra(raw, w, h, dw, dh))
            };

            // An encoder is bound to one frame size, so (re)build it whenever the
            // capture dimensions change.
            if self.enc_cfg.enabled
                && !self.enc_failed
                && self.encoder.as_ref().map(|e| e.dimensions()) != Some((w, h))
            {
                match HardwareEncoder::new(w, h, self.enc_cfg.fps, self.enc_cfg.bitrate_bps) {
                    Ok(e) => {
                        info!("GPU H.264 encoding at {w}x{h} — host receives NALs, not pixels");
                        self.encoder = Some(e);
                    }
                    Err(e) => {
                        warn!("no GPU encoder ({e:#}) — falling back to raw BGRA");
                        self.enc_failed = true;
                        self.encoder = None;
                    }
                }
            }

            if let Some(encoder) = self.encoder.as_mut() {
                if IDR_REQUESTED.swap(false, Ordering::Relaxed) {
                    encoder.request_keyframe();
                }
                match encoder.encode_bgra(&pixels) {
                    Ok(frames) => {
                        for f in frames {
                            match self.tx.try_send((
                                w,
                                h,
                                f.data,
                                FrameFormat::H264,
                                f.keyframe,
                            )) {
                                Ok(()) => self.sent += 1,
                                Err(_) => self.dropped += 1,
                            }
                        }
                    }
                    Err(e) => {
                        // One bad frame is not worth killing the stream over, but a
                        // broken encoder is: fall back rather than emit nothing.
                        warn!("hardware encode failed ({e:#}) — reverting to raw BGRA");
                        self.enc_failed = true;
                        self.encoder = None;
                    }
                }
                return Ok(());
            }

            match self.tx.try_send((w, h, pixels, FrameFormat::Bgra, true)) {
                Ok(()) => self.sent += 1,
                Err(_) => self.dropped += 1,
            }
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

    /// One byte per request; anything else means the link died.
    fn watch_for_idr_requests(mut reader: TcpStream) {
        use std::io::Read;
        let mut byte = [0u8; 1];
        loop {
            match reader.read(&mut byte) {
                Ok(0) | Err(_) => return,
                Ok(_) => {
                    if byte[0] == couchlink_capture_bridge::REQUEST_IDR {
                        IDR_REQUESTED.store(true, Ordering::Relaxed);
                    }
                }
            }
        }
    }

    fn spawn_tcp_writer(connect: String, rx: mpsc::Receiver<FrameMsg>) {
        std::thread::spawn(move || loop {
            match TcpStream::connect(&connect) {
                Ok(stream) => {
                    let _ = stream.set_nodelay(true);
                    info!("connected to {connect}");
                    // The host writes back on this socket to request keyframes; read
                    // it on its own thread so writes never block on it.
                    if let Ok(reader) = stream.try_clone() {
                        std::thread::spawn(move || watch_for_idr_requests(reader));
                    }
                    let mut writer = BufWriter::new(stream);
                    while let Ok((w, h, payload, format, keyframe)) = rx.recv() {
                        if let Err(e) =
                            write_frame_with_format(&mut writer, w, h, format, keyframe, &payload)
                        {
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

        // Depth 1, not 2: a queued frame is a frame the viewer will see late. With
        // depth 2 a frame could sit behind another for a whole send time before it
        // even reached the socket. Dropping the newest when busy costs a frame;
        // queueing it costs latency on every frame after it.
        let (tx, rx) = mpsc::sync_channel::<FrameMsg>(1);
        let frame_dur = Duration::from_millis(1000 / args.max_fps.max(1) as u64);
        // Encoding on the GPU here rather than on the host removes both the software
        // encoder and almost all of the wire cost; if anything about it fails we
        // simply keep sending raw pixels as before. Built on the capture thread.
        let enc_cfg = EncoderConfig {
            enabled: args.gpu_encode,
            fps: args.max_fps,
            bitrate_bps: args.bitrate_kbps * 1000,
        };
        if !enc_cfg.enabled {
            info!("GPU encoding disabled by flag — sending raw BGRA");
        }
        info!(
            "capturing at most {}x{} (wire format decided on the first frame)",
            args.max_width, args.max_height
        );
        let flags = (tx, frame_dur, args.max_width, args.max_height, enc_cfg);

        match args.source {
            CaptureSource::Desktop => {
                spawn_tcp_writer(args.connect.clone(), rx);
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
                let picked = item
                    .item
                    .DisplayName()
                    .map(|s| s.to_string())
                    .unwrap_or_else(|_| "<unknown>".into());
                let (pw, ph) = item.size().unwrap_or((0, 0));
                info!(
                    "picker selection accepted: '{picked}' {pw}x{ph} → {}",
                    args.connect
                );
                spawn_tcp_writer(args.connect.clone(), rx);
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
                if args.keep_rendering {
                    couchlink_capture_bridge::keep_rendering::spawn(w.as_raw_hwnd());
                }
                spawn_tcp_writer(args.connect.clone(), rx);
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
                let result = BridgeCapture::start(settings).map_err(|e| anyhow::anyhow!("{e}"));
                // Never leave someone else's window parked at -32000.
                couchlink_capture_bridge::keep_rendering::stop();
                result?;
            }
        }
        Ok(())
    }
}

#[cfg(windows)]
fn main() -> anyhow::Result<()> {
    run::main()
}
