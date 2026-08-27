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
    use couchlink_capture_bridge::gpu_convert::{self, GpuConverter, ReplayTarget};
    use couchlink_capture_bridge::mf_encoder::{EncoderRequest, HardwareEncoder};
    use windows::Win32::Graphics::Direct3D11::{ID3D11Device, ID3D11Texture2D};
    use couchlink_capture_bridge::{window_matches, write_frame_with_format, FrameFormat};
    use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
    use std::io::BufWriter;
    use std::net::TcpStream;
    use std::sync::{mpsc, Arc};
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
        #[arg(long, default_value_t = 1920)]
        pub max_width: u32,
        #[arg(long, default_value_t = 1080)]
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
        #[arg(long, default_value_t = 18000)]
        pub bitrate_kbps: u32,
    }

    /// width, height, payload, format, keyframe
    type FrameMsg = (u32, u32, Vec<u8>, FrameFormat, bool);

    /// What the capture thread hands the encoder. The texture variant never touches
    /// system memory; the pixel variant is the fallback that works everywhere.
    pub enum Surface {
        /// NV12 already on the GPU, converted by the video processor.
        Texture(SendTexture),
        Pixels(Vec<u8>),
    }

    /// SAFETY: the D3D11 device is created without D3D11_CREATE_DEVICE_SINGLETHREADED,
    /// so its objects are safe to use from another thread; the runtime serialises
    /// access. This only crosses from the capture thread to the encoder thread.
    pub struct SendTexture(pub ID3D11Texture2D);
    unsafe impl Send for SendTexture {}

    /// Same reasoning as SendTexture: a multithreaded D3D11 device may be used from
    /// any thread, and this only moves it to the encoder thread once at startup.
    pub struct SendDevice(pub ID3D11Device);
    unsafe impl Send for SendDevice {}

    #[derive(Clone, Copy)]
    pub struct EncoderCfg {
        pub enabled: bool,
    }

    /// Set by the socket reader when the host asks for an IDR (a player joined and
    /// needs something it can decode from scratch). Read by the capture thread,
    /// which owns the encoder.
    static IDR_REQUESTED: AtomicBool = AtomicBool::new(false);
    static EXPEDITE_ONCE: AtomicBool = AtomicBool::new(false);

    /// The encode target the host wants us to match. Written only by the socket
    /// reader when a SET_TARGET command arrives; read by the capture thread on
    /// every frame (fit box) and by the encoder thread at build time (fps/bitrate).
    ///
    /// Seeded from CLI args at startup so a host that never sends the command gets
    /// its old behaviour exactly — the command is a *correction*, not a necessity.
    static TARGET_W: AtomicU32 = AtomicU32::new(1920);
    static TARGET_H: AtomicU32 = AtomicU32::new(1080);
    static TARGET_FPS: AtomicU32 = AtomicU32::new(60);
    static TARGET_BITRATE_KBPS: AtomicU32 = AtomicU32::new(18000);

    fn apply_target(t: couchlink_capture_bridge::EncodeTarget) {
        if t.width == 0 || t.height == 0 || t.fps == 0 {
            warn!("ignoring malformed SET_TARGET {t:?}");
            return;
        }
        TARGET_W.store(t.width, Ordering::Relaxed);
        TARGET_H.store(t.height, Ordering::Relaxed);
        TARGET_FPS.store(t.fps, Ordering::Relaxed);
        TARGET_BITRATE_KBPS.store(t.bitrate_kbps, Ordering::Relaxed);
        info!(
            "host commanded encode target: {}x{}@{} ({} kbps)",
            t.width, t.height, t.fps, t.bitrate_kbps
        );
        // A target change is exactly when the frame size / GOP may change; start
        // the new encoder with a keyframe so the decoder never hangs on P-frames.
        IDR_REQUESTED.store(true, Ordering::Relaxed);
    }

    /// Set when GPU encoding is unavailable or has failed, switching the capture
    /// thread back to shipping raw pixels. Latched: we do not retry COM setup on
    /// every frame.
    static GPU_FALLBACK: AtomicBool = AtomicBool::new(false);

    /// Cleared when the encoder turns out not to accept D3D11 textures, so the
    /// capture thread stops converting on the GPU and goes back to readback. Without
    /// this the two halves disagree and every frame fails to submit.
    static ZERO_COPY_OK: AtomicBool = AtomicBool::new(true);
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
        raw_tx: Option<mpsc::SyncSender<(u32, u32, Surface, Instant)>>,
        /// None until the GPU conversion path is proven on the first frame.
        converter: Option<GpuConverter>,
        device: ID3D11Device,
        /// Latched once the GPU path is ruled out, so we stop retrying per frame.
        gpu_convert_failed: bool,
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
        device: SendDevice,
        out: mpsc::SyncSender<FrameMsg>,
    ) -> mpsc::SyncSender<(u32, u32, Surface, Instant)> {
        let (raw_tx, raw_rx) = mpsc::sync_channel::<(u32, u32, Surface, Instant)>(1);

        std::thread::spawn(move || {
            let mut seed: Option<(u32, u32, Surface, Instant)> = None;
            // Round-trip through the encoder for the frame currently being encoded,
            // so the reported latency is capture-to-encoded, not just encode time.
            let mut submitted_at: Option<Instant> = None;
            let mut enc_us: Vec<u64> = Vec::with_capacity(512);
            'build: loop {
                let Some((w, h, pixels, _t)) = seed.take().or_else(|| raw_rx.recv().ok()) else {
                    return;
                };
                // The host may have commanded fps/bitrate since the last build —
                // re-read them so a SET_TARGET lands on the next encoder, and mark
                // what we built so a later change rebuilds again.
                let fps = TARGET_FPS.load(Ordering::Relaxed).max(1);
                let bitrate_bps = TARGET_BITRATE_KBPS.load(Ordering::Relaxed) * 1000;
                // Prefer the device-backed encoder so textures can go straight in;
                // fall back to the system-memory encoder if it will not take a device.
                let zero_copy_wanted = matches!(pixels, Surface::Texture(_));
                let built = if zero_copy_wanted {
                    HardwareEncoder::new_with_device(&device.0, w, h, fps, bitrate_bps).or_else(|e| {
                        warn!("encoder refused the D3D11 device ({e:#}) — system memory it is");
                        HardwareEncoder::new(w, h, fps, bitrate_bps)
                    })
                } else {
                    HardwareEncoder::new(w, h, fps, bitrate_bps)
                };
                let mut encoder = match built {
                    Ok(e) => {
                        if !e.is_zero_copy() {
                            ZERO_COPY_OK.store(false, Ordering::Relaxed);
                        }
                        info!(
                            "GPU H.264 encoding at {w}x{h}@{fps} ({bitrate_bps} bps rate budget — {}) — host receives NALs, not pixels",
                            if e.is_zero_copy() { "zero-copy textures" } else { "system memory" }
                        );
                        e
                    }
                    Err(e) => {
                        warn!("no GPU encoder ({e:#}) — falling back to raw BGRA");
                        GPU_FALLBACK.store(true, Ordering::Relaxed);
                        return;
                    }
                };
                let mut latest: Option<(u32, u32, Surface, Instant)> =
                    Some((w, h, pixels, Instant::now()));
                // Feed the encoder on a fixed beat rather than whenever WGC happens
                // to deliver. Output cadence follows input cadence, and a receiver
                // sizes its jitter buffer from irregularity, not from rate — this is
                // the same fix that took the raw path's buffer from 97ms to 6ms.
                let tick = Duration::from_micros(1_000_000 / fps.max(1) as u64);
                let mut next_submit = Instant::now();
                // Only pixel frames can be re-encoded to fill a gap: a pooled texture
                // is recycled by the capture thread and would be overwritten.
                let mut previous: Option<(u32, u32, Vec<u8>, Instant)> = None;
                let mut replay_target: Option<ReplayTarget> = None;
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
                            if EXPEDITE_ONCE.swap(false, Ordering::Relaxed) {
                                next_submit = now;
                            }
                            if next_submit > now {
                                std::thread::sleep(next_submit - now);
                            }
                            next_submit = Instant::now() + tick;

                            // Take the newest frame available, not the oldest.
                            while let Ok(newer) = raw_rx.try_recv() {
                                latest = Some(newer);
                            }
                            // Nothing new this beat? Re-encode the frame we already
                            // have, so a static screen keeps the cadence hole-free
                            // for a few hundred bytes. Only pixel frames can be
                            // replayed: a pooled texture gets recycled by the capture
                            // thread and would be overwritten underneath us.
                            //
                            // A replayed frame carries no new content, so its age is
                            // not latency anyone perceives — only frames that actually
                            // changed are timed, or the number would measure how idle
                            // the desktop is rather than how responsive we are.
                            // Replay from a texture this thread owns outright.
                            //
                            // The converter's pool cannot be replayed: the capture
                            // thread rotates through it on every WGC frame regardless
                            // of what the encoder is doing, so a pooled texture held
                            // for replay gets recycled and overwritten mid-encode —
                            // that encoded garbage, and cost 609 decode errors and
                            // 4.9fps before it was caught.
                            let replay = replay_target
                                .as_ref()
                                .map(|r: &ReplayTarget| {
                                    let (w, h) = r.dimensions();
                                    (
                                        w,
                                        h,
                                        Surface::Texture(SendTexture(r.texture().clone())),
                                        Instant::now(),
                                    )
                                })
                                .or_else(|| {
                                    previous
                                        .clone()
                                        .map(|(w, h, px, t)| (w, h, Surface::Pixels(px), t))
                                });
                            let (from_source, next) = match latest.take() {
                                Some(f) => (true, Some(f)),
                                None => match replay {
                                    Some(r) => (false, Some(r)),
                                    // Blocking here still yields a genuinely new frame,
                                    // so it counts as fresh for timing.
                                    None => (true, raw_rx.recv().ok()),
                                },
                            };
                            let Some((fw, fh, surface, captured_at)) = next else {
                                return;
                            };
                            submitted_at = from_source.then_some(captured_at);
                            if (fw, fh) != encoder.dimensions() {
                                info!("capture resized to {fw}x{fh} — rebuilding the encoder");
                                seed = Some((fw, fh, surface, captured_at));
                                continue 'build;
                            }
                            // A SET_TARGET can change fps/bitrate without changing the
                            // frame size; the MFT is bound to one rate config at build,
                            // so rebuild then too.
                            let now_fps = TARGET_FPS.load(Ordering::Relaxed).max(1);
                            let now_kbps = TARGET_BITRATE_KBPS.load(Ordering::Relaxed);
                            if now_fps != fps || now_kbps * 1000 != bitrate_bps {
                                info!(
                                    "encode target now {fw}x{fh}@{now_fps} ({now_kbps} kbps) — rebuilding encoder"
                                );
                                seed = Some((fw, fh, surface, captured_at));
                                continue 'build;
                            }
                            let submitted = match &surface {
                                // A texture arriving at a system-memory encoder means
                                // the two halves disagreed; drop it and let the capture
                                // thread notice the flag rather than fail the encode.
                                Surface::Texture(_) if !encoder.is_zero_copy() => Ok(()),
                                Surface::Texture(t) => {
                                    let submitted = encoder.submit_texture(&t.0);
                                    // Only copy a frame that came from the capture
                                    // thread. On a replay `t` IS the replay texture,
                                    // and copying a resource onto itself is undefined —
                                    // comparing Rust references does not catch that,
                                    // since two clones of the same COM object live at
                                    // different addresses.
                                    if submitted.is_ok() && from_source {
                                        if replay_target.as_ref().map(|r| r.dimensions())
                                            != Some((fw, fh))
                                        {
                                            replay_target =
                                                ReplayTarget::new(&device.0, fw, fh).ok();
                                        }
                                        if let Some(r) = &mut replay_target {
                                            r.store(&t.0);
                                        }
                                    }
                                    submitted
                                }
                                Surface::Pixels(px) => {
                                    previous = Some((fw, fh, px.clone(), captured_at));
                                    encoder.submit(px)
                                }
                            };
                            if let Err(e) = submitted {
                                warn!("encoder submit failed ({e:#}) — falling back to raw BGRA");
                                GPU_FALLBACK.store(true, Ordering::Relaxed);
                                return;
                            }
                        }
                        Ok(EncoderRequest::HaveOutput(frames)) => {
                            if let Some(t) = submitted_at.take() {
                                enc_us.push(t.elapsed().as_micros() as u64);
                            }
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
                                    enc_us.sort_unstable();
                                    let p = |q: usize| {
                                        enc_us
                                            .get((enc_us.len() * q / 100).min(enc_us.len().saturating_sub(1)))
                                            .copied()
                                            .unwrap_or(0) as f64
                                            / 1000.0
                                    };
                                    info!(
                                        "encoded {:.1} fps ({bytes} bytes/frame, {stalled} not queued) \
                                         | capture->encoded p50={:.1}ms p99={:.1}ms",
                                        encoded as f64 / encoded_window.elapsed().as_secs_f64(),
                                        p(50),
                                        p(99)
                                    );
                                    enc_us.clear();
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

    /// Area-average box fit. Nearest-neighbour made UI text look crunchy whenever
    /// the capture was smaller than the monitor; this keeps edges readable without
    /// a heavyweight scaler on the capture thread.
    fn downscale_bgra(src: &[u8], sw: u32, sh: u32, dw: u32, dh: u32) -> Vec<u8> {
        let mut out = vec![0u8; (dw * dh * 4) as usize];
        let sw = sw as usize;
        let sh = sh as usize;
        let dw = dw as usize;
        let dh = dh as usize;
        for y in 0..dh {
            let y0 = y * sh / dh;
            let y1 = ((y + 1) * sh / dh).max(y0 + 1).min(sh);
            for x in 0..dw {
                let x0 = x * sw / dw;
                let x1 = ((x + 1) * sw / dw).max(x0 + 1).min(sw);
                let mut b = 0u32;
                let mut g = 0u32;
                let mut r = 0u32;
                let mut a = 0u32;
                let mut n = 0u32;
                for sy in y0..y1 {
                    let row = sy * sw;
                    for sx in x0..x1 {
                        let si = (row + sx) * 4;
                        if let Some(px) = src.get(si..si + 4) {
                            b += px[0] as u32;
                            g += px[1] as u32;
                            r += px[2] as u32;
                            a += px[3] as u32;
                            n += 1;
                        }
                    }
                }
                let di = (y * dw + x) * 4;
                if n > 0 {
                    out[di] = (b / n) as u8;
                    out[di + 1] = (g / n) as u8;
                    out[di + 2] = (r / n) as u8;
                    out[di + 3] = (a / n) as u8;
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
        type Flags = (mpsc::SyncSender<FrameMsg>, Duration, u32, u32, EncoderCfg);
        type Error = CaptureError;

        fn new(ctx: Context<Self::Flags>) -> Result<Self, Self::Error> {
            let cfg = ctx.flags.4;
            Ok(Self {
                tx: ctx.flags.0.clone(),
                frame_dur: ctx.flags.1,
                last: Instant::now() - Duration::from_secs(1),
                scratch: Vec::new(),
                max_w: ctx.flags.2,
                max_h: ctx.flags.3,
                // Spawned here rather than in main: the encoder needs the very device
                // the capture runs on, and that only exists once capture has started.
                raw_tx: cfg.enabled.then(|| {
                    spawn_encoder_thread(
                        SendDevice(ctx.device.clone()),
                        ctx.flags.0.clone(),
                    )
                }),
                converter: None,
                device: ctx.device.clone(),
                gpu_convert_failed: false,
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
            // The host may have commanded a resolution since this capture started;
            // re-read the target every frame so a resize takes effect promptly.
            self.max_w = TARGET_W.load(Ordering::Relaxed);
            self.max_h = TARGET_H.load(Ordering::Relaxed);
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
            let (w, h, _) = gpu_convert::texture_size(frame.as_raw_texture());
            // Zero-copy first: if the captured surface can be converted to NV12 on
            // the GPU, the pixels never touch system memory. This only applies when
            // no downscale is needed — scaling still happens on the CPU path, and
            // adding it to the video processor is a separate change.
            if self.raw_tx.is_some()
                && !self.gpu_convert_failed
                && ZERO_COPY_OK.load(Ordering::Relaxed)
            {
                let (tw, th) = fit(w, h, self.max_w, self.max_h);
                let texture = frame.as_raw_texture().clone();
                if gpu_convert::is_bgra(&texture) {
                    if self.converter.as_ref().map(|c| c.dimensions())
                        != Some(((w, h), (tw, th)))
                    {
                        match GpuConverter::new(&self.device, w, h, tw, th) {
                            Ok(c) => {
                                info!(
                                    "GPU scale+convert active: {w}x{h} -> {tw}x{th} NV12, no readback"
                                );
                                self.converter = Some(c);
                            }
                            Err(e) => {
                                warn!("no GPU colour conversion ({e:#}) — using CPU readback");
                                self.gpu_convert_failed = true;
                            }
                        }
                    }
                    if let Some(converter) = self.converter.as_mut() {
                        match converter.to_nv12(&texture) {
                            Ok(nv12) => {
                                let raw = self.raw_tx.as_ref().expect("checked above");
                                match raw.try_send((
                                    tw,
                                    th,
                                    Surface::Texture(SendTexture(nv12)),
                                    Instant::now(),
                                )) {
                                    Ok(()) => self.sent += 1,
                                    Err(_) => self.dropped += 1,
                                }
                                return Ok(());
                            }
                            Err(e) => {
                                warn!("GPU conversion failed ({e:#}) — using CPU readback");
                                self.gpu_convert_failed = true;
                                self.converter = None;
                            }
                        }
                    }
                }
            }

            let buffer = frame.buffer()?;
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
                    match raw.try_send((w, h, Surface::Pixels(pixels), Instant::now())) {
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

    /// Title *or* process name, and keep looking until it exists.
    ///
    /// Emulators rewrite the window title to the game name, so a title-only
    /// match for "PCSX2" misses "Marvel - Ultimate Alliance" owned by
    /// pcsx2-qt. Waiting means the host can start before the emulator does.
    fn wait_for_window(needle: &str) -> Result<Window> {
        let mut attempt = 0u32;
        loop {
            if let Some(w) = find_window(needle)? {
                return Ok(w);
            }
            attempt += 1;
            if attempt == 1 || attempt % 15 == 0 {
                warn!("no window matching {needle:?} by title or process — waiting");
            }
            std::thread::sleep(Duration::from_secs(2));
        }
    }

    fn find_window(needle: &str) -> Result<Option<Window>> {
        let wins = Window::enumerate().context("enumerate windows")?;
        for w in wins {
            let title = w.title().unwrap_or_default();
            let proc = w.process_name().unwrap_or_default();
            if window_matches(needle, &title, &proc) && !title.trim().is_empty() {
                return Ok(Some(w));
            }
        }
        Ok(None)
    }

    /// Host commands, multiplexed on the reverse of the frame socket.
    ///
    /// `I` = request IDR (1 byte). `T` = SET_TARGET, followed by 16 bytes of
    /// `EncodeTarget` (4 × `u32` LE). The reader blocks on the 16 bytes once it
    /// sees a `T`, which is fine: the host writes a command in one `write_all`,
    /// so a partial `T` payload only happens on a torn socket, which is broken
    /// anyway. Anything else means the link died.
    fn watch_commands(mut reader: impl std::io::Read) {
        let mut byte = [0u8; 1];
        loop {
            match reader.read(&mut byte) {
                Ok(0) | Err(_) => return,
                Ok(_) => match byte[0] {
                    couchlink_capture_bridge::REQUEST_IDR => {
                        IDR_REQUESTED.store(true, Ordering::Relaxed);
                    }
                    couchlink_capture_bridge::EXPEDITE => {
                        EXPEDITE_ONCE.store(true, Ordering::Relaxed);
                    }
                    couchlink_capture_bridge::SET_TARGET => {
                        let mut body = [0u8; 16];
                        if reader.read_exact(&mut body).is_err() {
                            return;
                        }
                        let u32le = |i: usize| u32::from_le_bytes(body[i..i + 4].try_into().unwrap());
                        apply_target(couchlink_capture_bridge::EncodeTarget {
                            width: u32le(0),
                            height: u32le(4),
                            fps: u32le(8),
                            bitrate_kbps: u32le(12),
                        });
                    }
                    _ => {} // unknown one-byte command: do not tear down capture
                },
            }
        }
    }

    fn spawn_tcp_writer(connect: String, rx: mpsc::Receiver<FrameMsg>) {
        // `hyperv:<port>:<vm-id>` skips TCP (and the whole WSL2 virtual
        // switch/NAT hop) in favour of a Hyper-V socket — see
        // `couchlink_capture_bridge::hyperv`. `vm-id` is the WSL2 side's own
        // `wslinfo --vm-id`: binding AF_HYPERV to the wildcard VmId instead
        // was tried first and does not work (see that module's docs).
        if let Some(rest) = connect.strip_prefix("hyperv:") {
            let Some((port, vm_id)) = rest.split_once(':') else {
                warn!("bad hyperv spec {rest:?} — expected hyperv:<port>:<vm-id>, exiting writer");
                return;
            };
            let Ok(port) = port.parse::<u32>() else {
                warn!("bad hyperv port {port:?} (expected a number), exiting writer");
                return;
            };
            let Ok(vm_id) = windows::core::GUID::try_from(vm_id) else {
                warn!("bad hyperv vm-id {vm_id:?} (expected a GUID), exiting writer");
                return;
            };
            spawn_hyperv_writer(port, vm_id, rx);
            return;
        }
        std::thread::spawn(move || loop {
            match TcpStream::connect(&connect) {
                Ok(stream) => {
                    let _ = stream.set_nodelay(true);
                    info!("connected to {connect}");
                    // The host writes back on this socket to request keyframes; read
                    // it on its own thread so writes never block on it. EOF on that
                    // reader means the host is gone — break the write loop so we
                    // reconnect instead of filling the queue forever.
                    let peer_alive = Arc::new(AtomicBool::new(true));
                    let flag = Arc::clone(&peer_alive);
                    if let Ok(reader) = stream.try_clone() {
                        std::thread::spawn(move || {
                            watch_commands(reader);
                            flag.store(false, Ordering::SeqCst);
                        });
                    }
                    // Shutdown unblocks a stuck write when the host drops.
                    let shutdown_stream = stream.try_clone().ok();
                    let mut writer = BufWriter::new(stream);
                    loop {
                        if !peer_alive.load(Ordering::SeqCst) {
                            info!("host gone on {connect} — reconnecting");
                            if let Some(s) = shutdown_stream.as_ref() {
                                let _ = s.shutdown(std::net::Shutdown::Both);
                            }
                            break;
                        }
                        match rx.recv_timeout(Duration::from_millis(50)) {
                            Ok((w, h, payload, format, keyframe)) => {
                                if let Err(e) = write_frame_with_format(
                                    &mut writer, w, h, format, keyframe, &payload,
                                ) {
                                    warn!("send frame failed: {e:#} — reconnecting");
                                    break;
                                }
                            }
                            Err(mpsc::RecvTimeoutError::Timeout) => continue,
                            Err(mpsc::RecvTimeoutError::Disconnected) => return,
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

    /// Listens on a Hyper-V socket and serves whichever WSL2 host connects.
    /// One listener, re-accepted forever, mirroring the TCP writer's own
    /// reconnect-and-keep-serving behaviour.
    ///
    /// Critical: when the WSL host drops the vsock, `write` often keeps
    /// succeeding into the Hyper-V buffer while `accept` never runs again —
    /// host logs "link lost", capture logs "queue full", and
    /// `ensure-win-capture` leaves the stuck process alone. Tear down the
    /// write loop as soon as the command reader sees EOF so we re-accept.
    fn spawn_hyperv_writer(port: u32, vm_id: windows::core::GUID, rx: mpsc::Receiver<FrameMsg>) {
        use couchlink_capture_bridge::hyperv::HvListener;
        use std::sync::atomic::{AtomicBool, Ordering};
        std::thread::spawn(move || {
            let listener = match HvListener::bind(port, vm_id) {
                Ok(l) => l,
                Err(e) => {
                    warn!("could not bind Hyper-V socket on port {port}: {e:#}");
                    return;
                }
            };
            loop {
                match listener.accept() {
                    Ok(mut stream) => {
                        info!("Hyper-V socket: WSL2 host connected");
                        let peer_alive = Arc::new(AtomicBool::new(true));
                        let flag = Arc::clone(&peer_alive);
                        let reader = stream.try_clone();
                        std::thread::spawn(move || {
                            watch_commands(reader.try_clone());
                            // Unblock a send() stuck on the half-open peer so
                            // the writer loop can re-accept the next host.
                            reader.shutdown();
                            flag.store(false, Ordering::SeqCst);
                        });
                        loop {
                            if !peer_alive.load(Ordering::SeqCst) {
                                info!("Hyper-V socket: WSL2 host gone — accepting again");
                                break;
                            }
                            match rx.recv_timeout(Duration::from_millis(50)) {
                                Ok((w, h, payload, format, keyframe)) => {
                                    if let Err(e) = write_frame_with_format(
                                        &mut stream,
                                        w,
                                        h,
                                        format,
                                        keyframe,
                                        &payload,
                                    ) {
                                        warn!(
                                            "send frame over Hyper-V socket failed: {e:#} — reconnecting"
                                        );
                                        break;
                                    }
                                }
                                Err(mpsc::RecvTimeoutError::Timeout) => continue,
                                Err(mpsc::RecvTimeoutError::Disconnected) => return,
                            }
                        }
                        stream.shutdown();
                    }
                    Err(e) => {
                        warn!("Hyper-V socket accept failed: {e:#}");
                        std::thread::sleep(Duration::from_millis(750));
                    }
                }
            }
        });
    }

    pub fn main() -> Result<()> {
        tracing_subscriber::fmt()
            .with_env_filter(
                tracing_subscriber::EnvFilter::try_from_default_env()
                    // The encoder lives in the library crate, so filtering on the
                    // binary alone silently discards everything it reports —
                    // including the warnings that explain a broken stream.
                    .unwrap_or_else(|_| {
                        "couchlink_win_capture=info,couchlink_capture_bridge=info".into()
                    }),
            )
            .init();

        let args = Args::parse();
        if args.list_windows {
            return list_windows();
        }
        // CLI args are the fallback the host's SET_TARGET supersedes.
        TARGET_W.store(args.max_width, Ordering::Relaxed);
        TARGET_H.store(args.max_height, Ordering::Relaxed);
        TARGET_FPS.store(args.max_fps, Ordering::Relaxed);
        TARGET_BITRATE_KBPS.store(args.bitrate_kbps, Ordering::Relaxed);

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
        let enc_cfg = EncoderCfg {
            enabled: args.gpu_encode,
        };
        if !enc_cfg.enabled {
            info!("GPU encoding disabled by flag — sending raw BGRA");
        }
        info!(
            "capturing at most {}x{} (wire format settles on the first frame)",
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
                // This process is launched from a background chain (WSL → cmd →
                // powershell → this exe), not from the user clicking something —
                // Windows' foreground-lock timeout denies a window from a process
                // like that the right to steal focus, so the picker's owner
                // window (crates.io windows-capture already does the correct
                // IInitializeWithWindow dance) can end up created but never
                // actually brought to the front: it opens invisibly behind
                // everything, and there is nothing to click. ASFW_ANY lifts that
                // restriction for the next SetForegroundWindow call from any
                // process, which is exactly what the picker's own window needs.
                unsafe {
                    let _ = windows::Win32::UI::WindowsAndMessaging::AllowSetForegroundWindow(
                        windows::Win32::UI::WindowsAndMessaging::ASFW_ANY,
                    );
                }
                info!("open the Windows capture picker and choose a window or monitor…");
                match GraphicsCapturePicker::pick_item().context("capture picker") {
                    Ok(Some(item)) => {
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
                    Ok(None) => {
                        warn!(
                            "capture picker dismissed — falling back to primary monitor \
                             (no window chosen)"
                        );
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
                    Err(e) => bail!("capture picker failed: {e:#}"),
                }
            }
            CaptureSource::Window => {
                if args.window.trim().is_empty() {
                    bail!("--source window requires --window TITLE_SUBSTRING");
                }
                let w = wait_for_window(&args.window)?;
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
