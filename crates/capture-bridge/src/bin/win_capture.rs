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
    use couchlink_capture_bridge::mf_encoder::{EncoderRequest, HardwareEncoder};
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
        /// to encode.
        ///
        /// Falls back to raw BGRA automatically if no hardware encoder exists or the
        /// transform fails; the host handles the format changing mid-stream.
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

    /// Set when GPU encoding is unavailable or has failed, switching the capture
    /// thread back to shipping raw pixels. Latched: we do not retry COM setup on
    /// every frame.
    static GPU_FALLBACK: AtomicBool = AtomicBool::new(false);
    type CaptureError = Box<dyn std::error::Error + Send + Sync>;

    struct BridgeCapture {
        tx: mpsc::SyncSender<FrameMsg>,
        frame_dur: Duration,
        last: Instant,
        scratch: Vec<u8>,
        max_w: u32,
        max_h: u32,
        /// Raw frames handed to the encoder thread. Depth 1: a newer frame replacing
        /// an unconsumed one is exactly what a live stream wants.
        raw_tx: Option<mpsc::SyncSender<(u32, u32, Vec<u8>)>>,
        arrived: u32,
        sent: u32,
        dropped: u32,
        rate_window: Instant,
    }

    /// Own the encoder on a dedicated thread parked on the MFT's event queue.
    ///
    /// An asynchronous (hardware) MFT posts METransformNeedInput / METransformHaveOutput
    /// on its own schedule and expects a caller blocked on GetEvent; polling it with
    /// MF_EVENT_FLAG_NO_WAIT returns MF_E_NO_EVENTS forever and nothing is ever encoded.
    ///
    /// The encoder is built from the first frame's dimensions rather than the
    /// requested maximum, because aspect-preserving fit means the real frame is
    /// usually smaller (a 1280x720 box holding a 16:10 monitor gives 1152x720), and
    /// an MFT is bound to exactly one frame size.
    fn spawn_encoder_thread(
        fps: u32,
        bitrate_bps: u32,
        out: mpsc::SyncSender<FrameMsg>,
    ) -> mpsc::SyncSender<(u32, u32, Vec<u8>)> {
        let (raw_tx, raw_rx) = mpsc::sync_channel::<(u32, u32, Vec<u8>)>(1);

        std::thread::spawn(move || {
            let mut seed: Option<(u32, u32, Vec<u8>)> = None;
            'build: loop {
                let Some((w, h, pixels)) = seed.take().or_else(|| raw_rx.recv().ok()) else {
                    return;
                };
                let mut encoder = match HardwareEncoder::new(w, h, fps, bitrate_bps) {
                    Ok(e) => {
                        info!("GPU H.264 encoding at {w}x{h} — host receives NALs, not pixels");
                        e
                    }
                    Err(e) => {
                        warn!("no GPU encoder ({e:#}) — falling back to raw BGRA");
                        GPU_FALLBACK.store(true, Ordering::Relaxed);
                        return;
                    }
                };
                let mut latest = Some((w, h, pixels));
                // Feed the encoder on a fixed beat rather than whenever WGC happens
                // to deliver. Output cadence follows input cadence, and a receiver
                // sizes its jitter buffer from irregularity, not from rate — this is
                // the same fix that took the raw path's buffer from 97ms to 6ms.
                let tick = Duration::from_micros(1_000_000 / fps.max(1) as u64);
                let mut next_submit = Instant::now();
                let mut previous: Option<(u32, u32, Vec<u8>)> = None;
                let mut encoded = 0u32;
                let mut stalled = 0u32;
                let mut encoded_window = Instant::now();

                loop {
                    if IDR_REQUESTED.swap(false, Ordering::Relaxed) {
                        encoder.request_keyframe();
                    }
                    match encoder.next_request() {
                        Ok(EncoderRequest::NeedInput) => {
                            // Hold the beat. Sleeping here is safe: the encoder is
                            // idle until fed, and the capture thread keeps replacing
                            // `latest` meanwhile, so we always submit the freshest
                            // frame rather than an older queued one.
                            let now = Instant::now();
                            if next_submit > now {
                                std::thread::sleep(next_submit - now);
                            }
                            next_submit = Instant::now() + tick;

                            // Take the newest frame available, not the oldest.
                            while let Ok(newer) = raw_rx.try_recv() {
                                latest = Some(newer);
                            }
                            // Nothing new this beat? Re-encode the frame we already
                            // have. A static screen costs a few hundred bytes and
                            // keeps the cadence hole-free, which is the entire point
                            // of a metronome; blocking here would make the cadence
                            // source-paced again.
                            let Some((fw, fh, px)) = latest
                                .take()
                                .or_else(|| previous.clone())
                                .or_else(|| raw_rx.recv().ok())
                            else {
                                return;
                            };
                            previous = Some((fw, fh, px.clone()));
                            if (fw, fh) != encoder.dimensions() {
                                info!("capture resized to {fw}x{fh} — rebuilding the encoder");
                                seed = Some((fw, fh, px));
                                continue 'build;
                            }
                            if let Err(e) = encoder.submit(&px) {
                                warn!("encoder submit failed ({e:#}) — falling back to raw BGRA");
                                GPU_FALLBACK.store(true, Ordering::Relaxed);
                                return;
                            }
                        }
                        Ok(EncoderRequest::HaveOutput(frames)) => {
                            let (w, h) = encoder.dimensions();
                            for f in frames {
                                let bytes = f.data.len();
                                let key = f.keyframe;
                                // Dropping an H.264 frame breaks every frame after it
                                // until the next keyframe, so a silent drop is not an
                                // option — but blocking is worse: the host does not
                                // read this socket until a player connects, so a
                                // blocking send parks the encoder indefinitely.
                                //
                                // Resolve it by making the drop safe: shed the frame to
                                // stay current, then immediately ask for an IDR so the
                                // decoder resynchronises on the very next frame instead
                                // of glitching until the next scheduled keyframe.
                                let queued = out
                                    .try_send((w, h, f.data, FrameFormat::H264, key))
                                    .is_ok();
                                encoded += 1;
                                if !queued {
                                    stalled += 1;
                                    encoder.request_keyframe();
                                }
                                if encoded_window.elapsed() >= Duration::from_secs(5) {
                                    info!(
                                        "encoded {:.1} fps ({bytes} bytes/frame, {stalled} not queued)",
                                        encoded as f64 / encoded_window.elapsed().as_secs_f64()
                                    );
                                    encoded = 0;
                                    stalled = 0;
                                    encoded_window = Instant::now();
                                }
                            }
                            latest = raw_rx.try_recv().ok();
                        }
                        Err(e) => {
                            warn!("encoder event loop ended ({e:#}) — falling back to raw BGRA");
                            GPU_FALLBACK.store(true, Ordering::Relaxed);
                            return;
                        }
                    }
                }
            }
        });
        raw_tx
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
        type Flags = (
            mpsc::SyncSender<FrameMsg>,
            Duration,
            u32,
            u32,
            Option<mpsc::SyncSender<(u32, u32, Vec<u8>)>>,
        );
        type Error = CaptureError;

        fn new(ctx: Context<Self::Flags>) -> Result<Self, Self::Error> {
            Ok(Self {
                tx: ctx.flags.0,
                frame_dur: ctx.flags.1,
                last: Instant::now() - Duration::from_secs(1),
                scratch: Vec::new(),
                max_w: ctx.flags.2,
                max_h: ctx.flags.3,
                raw_tx: ctx.flags.4,
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

            // The encoder thread may give up at any point (no hardware, a mid-stream
            // failure); when it does, this switches back to raw pixels rather than
            // stopping the stream.
            let pixels = match &self.raw_tx {
                Some(raw) if !GPU_FALLBACK.load(Ordering::Relaxed) => {
                    // A full queue means the GPU is still busy with the previous
                    // frame; dropping this one keeps latency flat.
                    match raw.try_send((w, h, pixels)) {
                        Ok(()) => self.sent += 1,
                        Err(_) => self.dropped += 1,
                    }
                    return Ok(());
                }
                _ => pixels,
            };

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

        // Depth is a latency/throughput trade and it depends on frame size. A raw
        // 3.3MB frame takes ~50ms to push, so queueing one costs real latency —
        // depth 1. An encoded frame is under 70KB and often under 1KB, so a couple
        // of slots cost microseconds and stop the encoder's output being thrown away
        // whenever the writer is mid-flush.
        let queue_depth = if args.gpu_encode { 2 } else { 1 };
        let (tx, rx) = mpsc::sync_channel::<FrameMsg>(queue_depth);
        let frame_dur = Duration::from_millis(1000 / args.max_fps.max(1) as u64);
        // Encoding on the GPU here rather than on the host removes both the software
        // encoder and almost all of the wire cost; if anything about it fails we
        // simply keep sending raw pixels as before.
        let raw_tx = if args.gpu_encode {
            Some(spawn_encoder_thread(
                args.max_fps,
                args.bitrate_kbps * 1000,
                tx.clone(),
            ))
        } else {
            info!("GPU encoding disabled by flag — sending raw BGRA");
            None
        };
        info!(
            "capturing at most {}x{} (wire format settles on the first frame)",
            args.max_width, args.max_height
        );
        let flags = (tx, frame_dur, args.max_width, args.max_height, raw_tx);

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
