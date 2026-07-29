//! Hardware H.264 encoding on Windows via a Media Foundation Transform.
//!
//! Encoding here instead of on the Linux host changes the economics of the whole
//! pipeline. A 720p BGRA frame is 3.3MB and costs the host ~10ms to convert and
//! encode in software; the same frame encoded on the GPU is tens of kilobytes and
//! costs the host nothing but a socket read. The WSL virtual NIC stops being a
//! factor and the software encoder disappears from the latency budget.
//!
//! Every step of setup can fail on a machine without a hardware encoder, so
//! construction returns Result and the caller is expected to fall back to sending
//! raw pixels rather than treating this as required.

use anyhow::{anyhow, bail, Context, Result};
use windows::core::{Interface, GUID, PWSTR};
use windows::Win32::Media::MediaFoundation::*;
use windows::Win32::System::Com::{CoInitializeEx, CoTaskMemFree, COINIT_MULTITHREADED};
use windows::Win32::Graphics::Direct3D11::ID3D11Texture2D;
use windows::Win32::System::Variant::{VARIANT, VT_UI4};

/// Annex-B start code. WebRTC wants byte-stream format; some encoders emit AVCC
/// (4-byte big-endian lengths) instead, which we convert.
const START_CODE: [u8; 4] = [0, 0, 0, 1];

/// MF_E_NO_EVENTS — the async MFT has nothing further to report right now. Not
/// exported by the windows crate, so spelled out here.
const NO_EVENTS: windows::core::HRESULT = windows::core::HRESULT(0xC00D3E80u32 as i32);

pub struct HardwareEncoder {
    transform: IMFTransform,
    codec_api: Option<ICodecAPI>,
    /// Hardware encoders are asynchronous MFTs: they will not accept a blocking
    /// ProcessInput/ProcessOutput pair and instead announce readiness through
    /// events. None for a synchronous (software) transform.
    events: Option<IMFMediaEventGenerator>,
    width: u32,
    height: u32,
    /// SPS/PPS, prepended to every keyframe that lacks them.
    ///
    /// A decoder cannot start without parameter sets, and MF encoders typically emit
    /// them only once at the start of the stream. Any viewer joining later then
    /// receives a stream it can never decode (openh264 reports dsNoParamSets), which
    /// is invisible to whoever was already connected.
    ///
    /// Read from MF_MT_MPEG_SEQUENCE_HEADER where available, and otherwise learned
    /// from the encoder's own output — that attribute is not always present.
    sequence_header: Vec<u8>,
    frame_index: i64,
    frame_duration: i64,
    started: std::time::Instant,
    nv12: Vec<u8>,
    /// Set once we have seen how this encoder formats its output.
    annex_b_confirmed: bool,
    /// True when the transform allocates its own output samples. When false we must
    /// hand it a buffer of at least `output_sample_size` on every ProcessOutput.
    provides_samples: bool,
    output_sample_size: u32,
    learned_parameter_sets: bool,
    /// Held for the lifetime of the transform when running zero-copy; dropping it
    /// would pull the device out from under the encoder.
    dxgi_manager: Option<IMFDXGIDeviceManager>,
}

// SAFETY: COM is initialised as a multithreaded apartment (COINIT_MULTITHREADED),
// in which interface pointers are valid from any MTA thread — no proxy marshalling
// required. In practice this encoder is also created and used exclusively on the
// capture thread; the bound exists only because the capture handler type must be
// Send. Do not weaken the CoInitializeEx call above without revisiting this.
unsafe impl Send for HardwareEncoder {}

/// What an asynchronous MFT is asking for.
pub enum EncoderRequest {
    NeedInput,
    HaveOutput(Vec<EncodedFrame>),
}

/// One encoded access unit.
pub struct EncodedFrame {
    pub data: Vec<u8>,
    pub keyframe: bool,
}

impl HardwareEncoder {
    /// `fps` is the rate frames are submitted, and it sets the floor on latency:
    /// the transform asks for input at roughly that rate, so a frame arriving at an
    /// arbitrary moment waits about half an interval before it is taken. Measured,
    /// capture->encoded tracks 1/(2*fps) + encode: ~12ms at 60, ~8ms at 120.
    ///
    /// Tried and rejected: raising MF_MT_FRAME_RATE above the submit rate to make the
    /// transform ask more often. It changes rate-control accounting and does not move
    /// the latency, because the request rate follows how fast the encoder actually
    /// drains, not the declared nominal rate.
    /// Zero-copy variant: the transform is given the D3D11 device the capture runs
    /// on, so it can read NV12 textures directly instead of us reading the surface
    /// back to system memory, converting on the CPU, and uploading it again.
    ///
    /// Returns an error if the transform is not D3D-aware or refuses the device, and
    /// the caller is expected to fall back to the system-memory path.
    pub fn new_with_device(
        device: &windows::Win32::Graphics::Direct3D11::ID3D11Device,
        width: u32,
        height: u32,
        fps: u32,
        bitrate_bps: u32,
    ) -> Result<Self> {
        Self::build(width, height, fps, bitrate_bps, Some(device))
    }

    /// True when frames may be submitted as textures.
    pub fn is_zero_copy(&self) -> bool {
        self.dxgi_manager.is_some()
    }

    /// Submit an NV12 texture the GPU already holds. No readback, no CPU conversion,
    /// no upload.
    pub fn submit_texture(&mut self, texture: &ID3D11Texture2D) -> Result<()> {
        unsafe {
            let buffer = MFCreateDXGISurfaceBuffer(
                &ID3D11Texture2D::IID,
                texture,
                0,
                false,
            )
            .context("MFCreateDXGISurfaceBuffer")?;
            // A DXGI-backed buffer knows its own size, but the transform still wants
            // the current length set.
            if let Ok(len) = buffer.GetMaxLength() {
                let _ = buffer.SetCurrentLength(len);
            }

            let sample = MFCreateSample().context("MFCreateSample")?;
            sample.AddBuffer(&buffer)?;
            let elapsed = self.started.elapsed();
            sample.SetSampleTime((elapsed.as_nanos() / 100) as i64)?;
            sample.SetSampleDuration(self.frame_duration)?;
            self.frame_index += 1;

            self.transform
                .ProcessInput(0, &sample, 0)
                .context("ProcessInput (texture)")?;
        }
        Ok(())
    }

    pub fn new(width: u32, height: u32, fps: u32, bitrate_bps: u32) -> Result<Self> {
        Self::build(width, height, fps, bitrate_bps, None)
    }

    fn build(
        width: u32,
        height: u32,
        fps: u32,
        bitrate_bps: u32,
        device: Option<&windows::Win32::Graphics::Direct3D11::ID3D11Device>,
    ) -> Result<Self> {
        if width % 2 != 0 || height % 2 != 0 {
            bail!("hardware encoder needs even dimensions, got {width}x{height}");
        }
        unsafe {
            // Ignore the result: the process may already be initialised, which is not
            // an error for our purposes.
            let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
            MFStartup(MF_VERSION, MFSTARTUP_LITE).context("MFStartup")?;
        }

        let transform = find_hardware_encoder()?;
        let fps = fps.max(1);

        // A hardware MFT stays locked until the caller declares it can drive the
        // asynchronous model; without this every SetInputType fails with
        // MF_E_TRANSFORM_ASYNC_LOCKED.
        let is_async = unsafe {
            match transform.GetAttributes() {
                Ok(attrs) => {
                    let is_async = attrs.GetUINT32(&MF_TRANSFORM_ASYNC).unwrap_or(0) == 1;
                    if is_async {
                        attrs
                            .SetUINT32(&MF_TRANSFORM_ASYNC_UNLOCK, 1)
                            .context("unlock async MFT")?;
                    }
                    is_async
                }
                Err(_) => false,
            }
        };

        // The device has to go in before the media types are negotiated: a transform
        // asked to accept a device after it has already been configured for system
        // memory answers with a bare E_FAIL that names nothing.
        let mut dxgi_manager = None;
        if let Some(device) = device {
            unsafe {
                let aware = transform
                    .GetAttributes()
                    .ok()
                    .and_then(|a| a.GetUINT32(&MF_SA_D3D11_AWARE).ok())
                    .unwrap_or(0);
                if aware == 0 {
                    bail!("encoder is not D3D11-aware; keeping the system-memory path");
                }
                // Media Foundation drives the device from its own threads and refuses
                // one that is not multithread-protected.
                let multithread: windows::Win32::Graphics::Direct3D10::ID3D10Multithread =
                    device.cast().context("device has no ID3D10Multithread")?;
                multithread.SetMultithreadProtected(true);

                let mut manager: Option<IMFDXGIDeviceManager> = None;
                let mut reset_token = 0u32;
                MFCreateDXGIDeviceManager(&mut reset_token, &mut manager)
                    .context("MFCreateDXGIDeviceManager")?;
                let manager = manager.context("no DXGI device manager")?;
                manager
                    .ResetDevice(device, reset_token)
                    .context("ResetDevice")?;
                transform
                    .ProcessMessage(MFT_MESSAGE_SET_D3D_MANAGER, manager.as_raw() as usize)
                    .context("MFT_MESSAGE_SET_D3D_MANAGER")?;
                tracing::info!("encoder accepted the D3D11 device — textures go in directly");
                dxgi_manager = Some(manager);
            }
        }

        unsafe {
            // Output type must be set before input type for encoder MFTs.
            let out = MFCreateMediaType().context("create output type")?;
            out.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)?;
            out.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_H264)?;
            out.SetUINT32(&MF_MT_AVG_BITRATE, bitrate_bps)?;
            out.SetUINT32(
                &MF_MT_INTERLACE_MODE,
                MFVideoInterlace_Progressive.0 as u32,
            )?;
            set_ratio(&out, &MF_MT_FRAME_SIZE, width, height)?;
            set_ratio(&out, &MF_MT_FRAME_RATE, fps, 1)?;
            set_ratio(&out, &MF_MT_PIXEL_ASPECT_RATIO, 1, 1)?;
            // Main is a big quality win over Baseline at the same bitrate (CABAC),
            // and Chrome/WebCodecs decode it fine. Fall back to Baseline if rejected.
            if out
                .SetUINT32(&MF_MT_MPEG2_PROFILE, eAVEncH264VProfile_Main.0 as u32)
                .is_err()
                || transform.SetOutputType(0, &out, 0).is_err()
            {
                tracing::warn!("Main profile rejected — falling back to Baseline");
                out.SetUINT32(&MF_MT_MPEG2_PROFILE, eAVEncH264VProfile_Base.0 as u32)?;
                transform
                    .SetOutputType(0, &out, 0)
                    .context("SetOutputType(H264) — encoder rejected these parameters")?;
            }

            let inp = MFCreateMediaType().context("create input type")?;
            inp.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)?;
            inp.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_NV12)?;
            inp.SetUINT32(
                &MF_MT_INTERLACE_MODE,
                MFVideoInterlace_Progressive.0 as u32,
            )?;
            set_ratio(&inp, &MF_MT_FRAME_SIZE, width, height)?;
            set_ratio(&inp, &MF_MT_FRAME_RATE, fps, 1)?;
            set_ratio(&inp, &MF_MT_PIXEL_ASPECT_RATIO, 1, 1)?;
            transform
                .SetInputType(0, &inp, 0)
                .context("SetInputType(NV12)")?;

            transform.ProcessMessage(MFT_MESSAGE_COMMAND_FLUSH, 0)?;
            transform.ProcessMessage(MFT_MESSAGE_NOTIFY_BEGIN_STREAMING, 0)?;
            transform.ProcessMessage(MFT_MESSAGE_NOTIFY_START_OF_STREAM, 0)?;
        }

        // Whether the transform allocates output samples decides how ProcessOutput
        // must be called. Getting this wrong is E_UNEXPECTED partway into a stream.
        let (provides_samples, output_sample_size) = unsafe {
            match transform.GetOutputStreamInfo(0) {
                Ok(info) => {
                    const PROVIDES: u32 = 0x100; // MFT_OUTPUT_STREAM_PROVIDES_SAMPLES
                    const CAN_PROVIDE: u32 = 0x200; // MFT_OUTPUT_STREAM_CAN_PROVIDE_SAMPLES
                    let provides = info.dwFlags & (PROVIDES | CAN_PROVIDE) != 0;
                    tracing::info!(
                        "output stream: flags={:#x} size={} provides_samples={}",
                        info.dwFlags,
                        info.cbSize,
                        provides
                    );
                    (provides, info.cbSize.max(1))
                }
                Err(e) => {
                    tracing::warn!("GetOutputStreamInfo failed ({e}); assuming self-allocating");
                    (true, 1)
                }
            }
        };

        let sequence_header = unsafe { read_sequence_header(&transform) };
        let codec_api: Option<ICodecAPI> = transform.cast().ok();
        if let Some(ref api) = codec_api {
            unsafe {
                apply_codec_api_defaults(api, bitrate_bps);
            }
        }
        let events: Option<IMFMediaEventGenerator> = if is_async {
            Some(
                transform
                    .cast()
                    .context("async MFT without an event generator")?,
            )
        } else {
            None
        };
        tracing::info!(
            "hardware encoder ready ({} model)",
            if is_async { "asynchronous" } else { "synchronous" }
        );

        Ok(Self {
            transform,
            codec_api,
            events,
            width,
            height,
            sequence_header,
            frame_index: 0,
            // Media Foundation time is in 100ns units.
            frame_duration: 10_000_000 / fps as i64,
            started: std::time::Instant::now(),
            nv12: vec![0u8; nv12_len(width, height)],
            annex_b_confirmed: false,
            provides_samples,
            output_sample_size,
            learned_parameter_sets: false,
            dxgi_manager,
        })
    }

    pub fn dimensions(&self) -> (u32, u32) {
        (self.width, self.height)
    }

    /// Ask for an IDR on the next frame. Used when a player joins mid-session.
    pub fn request_keyframe(&self) {
        let Some(api) = &self.codec_api else { return };
        unsafe {
            let _ = set_codec_u32(api, &CODECAPI_AVEncVideoForceKeyFrame, 1);
        }
    }

    /// Feed one BGRA frame, collect whatever the encoder is ready to emit. An
    /// encoder may return nothing for a frame (pipelining) or more than one.
    pub fn encode_bgra(&mut self, bgra: &[u8]) -> Result<Vec<EncodedFrame>> {
        unsafe {
            let sample = self.make_sample(bgra)?;
            match self.transform.ProcessInput(0, &sample, 0) {
                Ok(()) => {}
                Err(e) if e.code() == MF_E_NOTACCEPTING => {
                    // Encoder is full; drain and drop this frame rather than stalling
                    // the capture thread. A dropped frame beats a growing queue.
                    return self.drain();
                }
                Err(e) => return Err(e).context("ProcessInput"),
            }
        }
        self.drain()
    }

    pub fn is_async(&self) -> bool {
        self.events.is_some()
    }

    /// Block until the asynchronous MFT says what it wants next.
    ///
    /// Polling this queue with MF_EVENT_FLAG_NO_WAIT returns MF_E_NO_EVENTS forever —
    /// the transform posts events on its own schedule and expects a caller parked on
    /// the queue. Hence the dedicated encoder thread.
    pub fn next_request(&mut self) -> Result<EncoderRequest> {
        let events = self
            .events
            .clone()
            .ok_or_else(|| anyhow!("next_request on a synchronous transform"))?;
        loop {
            // Zero flags = block until an event is available, which is the whole
            // point of this thread.
            let event = match unsafe {
                events.GetEvent(MEDIA_EVENT_GENERATOR_GET_EVENT_FLAGS(0))
            } {
                Ok(e) => e,
                Err(e) if e.code() == NO_EVENTS => continue,
                Err(e) => return Err(anyhow!("GetEvent: {e}")),
            };
            let kind = unsafe { event.GetType()? };
            if kind == METransformNeedInput.0 as u32 {
                return Ok(EncoderRequest::NeedInput);
            }
            if kind == METransformHaveOutput.0 as u32 {
                // Exactly one ProcessOutput per HaveOutput event. An async MFT
                // returns E_UNEXPECTED for a second call it did not invite.
                let frame = self.process_output_once()?;
                return Ok(EncoderRequest::HaveOutput(frame.into_iter().collect()));
            }
            // Anything else (format change, drain complete) is not interesting here.
        }
    }

    /// Feed one frame to an asynchronous MFT that has just asked for input.
    pub fn submit(&mut self, bgra: &[u8]) -> Result<()> {
        let sample = unsafe { self.make_sample(bgra)? };
        unsafe {
            self.transform
                .ProcessInput(0, &sample, 0)
                .context("ProcessInput (async)")?;
        }
        Ok(())
    }

    unsafe fn alloc_output_sample(&self) -> Result<IMFSample> {
        let buffer = MFCreateMemoryBuffer(self.output_sample_size)
            .context("allocate output buffer")?;
        let sample = MFCreateSample().context("allocate output sample")?;
        sample.AddBuffer(&buffer)?;
        Ok(sample)
    }

    unsafe fn make_sample(&mut self, bgra: &[u8]) -> Result<IMFSample> {
        bgra_to_nv12(bgra, self.width as usize, self.height as usize, &mut self.nv12);
        let buffer =
            MFCreateMemoryBuffer(self.nv12.len() as u32).context("MFCreateMemoryBuffer")?;
        let mut dst = std::ptr::null_mut();
        let mut max = 0u32;
        buffer.Lock(&mut dst, Some(&mut max), None)?;
        std::ptr::copy_nonoverlapping(self.nv12.as_ptr(), dst, self.nv12.len());
        buffer.Unlock()?;
        buffer.SetCurrentLength(self.nv12.len() as u32)?;

        let sample = MFCreateSample().context("MFCreateSample")?;
        sample.AddBuffer(&buffer)?;
        // Real elapsed time rather than frame_index * nominal duration: frames are
        // submitted whenever the source produces them, so a frame counter drifts away
        // from the wall clock and rate control mis-allocates bits.
        let elapsed = self.started.elapsed();
        sample.SetSampleTime((elapsed.as_nanos() / 100) as i64)?;
        sample.SetSampleDuration(self.frame_duration)?;
        self.frame_index += 1;
        Ok(sample)
    }

    /// Synchronous transforms are drained until they ask for more input.
    fn drain(&mut self) -> Result<Vec<EncodedFrame>> {
        let mut out = Vec::new();
        while let Some(frame) = self.process_output_once()? {
            out.push(frame);
        }
        Ok(out)
    }

    /// Exactly one ProcessOutput call. `None` means the transform has nothing to give
    /// right now — it wants more input, or it just renegotiated its output type.
    fn process_output_once(&mut self) -> Result<Option<EncodedFrame>> {
        {
            // A transform that does not allocate its own samples expects one from us,
            // sized by GetOutputStreamInfo. Passing None to such a transform is what
            // produces E_UNEXPECTED once its internal pool is exhausted.
            let supplied = if self.provides_samples {
                None
            } else {
                Some(unsafe { self.alloc_output_sample()? })
            };
            let mut buffers = [MFT_OUTPUT_DATA_BUFFER {
                dwStreamID: 0,
                pSample: std::mem::ManuallyDrop::new(supplied),
                dwStatus: 0,
                pEvents: std::mem::ManuallyDrop::new(None),
            }];
            let mut status = 0u32;
            let hr = unsafe { self.transform.ProcessOutput(0, &mut buffers, &mut status) };

            if let Err(e) = hr {
                let sample = unsafe { std::mem::ManuallyDrop::take(&mut buffers[0].pSample) };
                drop(sample);
                if e.code() == MF_E_TRANSFORM_NEED_MORE_INPUT {
                    return Ok(None);
                }
                if e.code() == MF_E_TRANSFORM_STREAM_CHANGE {
                    // Renegotiate, but do NOT call ProcessOutput again here. On an
                    // asynchronous MFT every ProcessOutput must be invited by its own
                    // METransformHaveOutput event; an unsolicited second call is
                    // exactly what returns E_UNEXPECTED. Wait for the next event.
                    self.renegotiate_output()?;
                    return Ok(None);
                }
                return Err(anyhow!("ProcessOutput failed: {e}"));
            }

            let sample = unsafe { std::mem::ManuallyDrop::take(&mut buffers[0].pSample) };
            let Some(sample) = sample else { return Ok(None) };
            return Ok(Some(unsafe { self.read_sample(&sample)? }));
        }
    }

    /// After MF_E_TRANSFORM_STREAM_CHANGE the transform expects the output type to be
    /// set again before it will produce anything.
    fn renegotiate_output(&mut self) -> Result<()> {
        unsafe {
            if let Ok(ty) = self.transform.GetOutputAvailableType(0, 0) {
                let _ = self.transform.SetOutputType(0, &ty, 0);
            }
            self.sequence_header = read_sequence_header(&self.transform);
        }
        Ok(())
    }

    unsafe fn read_sample(&mut self, sample: &IMFSample) -> Result<EncodedFrame> {
        let buffer = sample.ConvertToContiguousBuffer()?;
        let mut ptr = std::ptr::null_mut();
        let mut len = 0u32;
        buffer.Lock(&mut ptr, None, Some(&mut len))?;
        let mut data = std::slice::from_raw_parts(ptr, len as usize).to_vec();
        buffer.Unlock()?;

        // MFSampleExtension_CleanPoint marks an IDR.
        let keyframe = sample
            .GetUINT32(&MFSampleExtension_CleanPoint)
            .map(|v| v != 0)
            .unwrap_or(false);

        if !data.starts_with(&START_CODE) && !data.starts_with(&START_CODE[1..]) {
            // AVCC: 4-byte big-endian lengths. Rewrite to Annex-B for WebRTC.
            data = avcc_to_annex_b(&data)?;
        } else if !self.annex_b_confirmed {
            self.annex_b_confirmed = true;
        }

        // Prefer parameter sets observed in the stream over the media type's blob.
        // MF_MT_MPEG_SEQUENCE_HEADER is sometimes an AVCDecoderConfigurationRecord
        // (AVCC, length-prefixed) rather than Annex-B; prepending that to a keyframe
        // feeds the decoder garbage and it reports "no parameter sets" forever.
        if !self.learned_parameter_sets {
            if let Some(sets) = extract_parameter_sets(&data) {
                tracing::info!(
                    "cached {} bytes of SPS/PPS from the stream (was {} from the media type)",
                    sets.len(),
                    self.sequence_header.len()
                );
                self.sequence_header = sets;
                self.learned_parameter_sets = true;
            }
        }
        if keyframe && !self.sequence_header.is_empty() && !contains_sps(&data) {
            let mut with_header = self.sequence_header.clone();
            with_header.extend_from_slice(&data);
            data = with_header;
        }

        Ok(EncodedFrame { data, keyframe })
    }
}

impl Drop for HardwareEncoder {
    fn drop(&mut self) {
        unsafe {
            let _ = self
                .transform
                .ProcessMessage(MFT_MESSAGE_NOTIFY_END_OF_STREAM, 0);
            let _ = self
                .transform
                .ProcessMessage(MFT_MESSAGE_NOTIFY_END_STREAMING, 0);
        }
    }
}

/// Enumerate hardware H.264 encoders and activate the first that works. Hardware
/// only: a software MFT here would just be OpenH264's problem wearing a different
/// hat, and the caller's fallback already covers that case.
fn find_hardware_encoder() -> Result<IMFTransform> {
    let input = MFT_REGISTER_TYPE_INFO {
        guidMajorType: MFMediaType_Video,
        guidSubtype: MFVideoFormat_NV12,
    };
    let output = MFT_REGISTER_TYPE_INFO {
        guidMajorType: MFMediaType_Video,
        guidSubtype: MFVideoFormat_H264,
    };

    unsafe {
        let mut activates: *mut Option<IMFActivate> = std::ptr::null_mut();
        let mut count = 0u32;
        MFTEnumEx(
            MFT_CATEGORY_VIDEO_ENCODER,
            MFT_ENUM_FLAG_HARDWARE | MFT_ENUM_FLAG_SORTANDFILTER,
            Some(&input),
            Some(&output),
            &mut activates,
            &mut count,
        )
        .context("MFTEnumEx")?;

        if count == 0 || activates.is_null() {
            bail!("no hardware H.264 encoder found on this machine");
        }

        let list = std::slice::from_raw_parts(activates, count as usize);
        let mut chosen = None;
        let mut last_err = None;
        for activate in list.iter().flatten() {
            let mut name_ptr = PWSTR::null();
            let mut name_len = 0u32;
            let name = match activate.GetAllocatedString(
                &MFT_FRIENDLY_NAME_Attribute,
                &mut name_ptr,
                &mut name_len,
            ) {
                Ok(()) => widestring_to_string(name_ptr, name_len),
                Err(_) => "<unnamed>".to_string(),
            };
            match activate.ActivateObject::<IMFTransform>() {
                Ok(t) => {
                    tracing::info!("hardware H.264 encoder: {name}");
                    chosen = Some(t);
                    break;
                }
                Err(e) => {
                    tracing::debug!("encoder {name} would not activate: {e}");
                    last_err = Some(e);
                }
            }
        }
        CoTaskMemFree(Some(activates as *const _));

        chosen.ok_or_else(|| match last_err {
            Some(e) => anyhow!("no hardware H.264 encoder could be activated: {e}"),
            None => anyhow!("no hardware H.264 encoder could be activated"),
        })
    }
}

unsafe fn widestring_to_string(p: PWSTR, len: u32) -> String {
    if p.is_null() {
        return "<unnamed>".into();
    }
    let s = String::from_utf16_lossy(std::slice::from_raw_parts(p.0, len as usize));
    CoTaskMemFree(Some(p.0 as *const _));
    s
}

/// Keep encode latency low while giving the rate controller enough bits for UI text.
unsafe fn apply_codec_api_defaults(api: &ICodecAPI, bitrate_bps: u32) {
    // Low-latency mode must stay on — quality bumps must not reintroduce encoder delay.
    let _ = set_codec_u32(api, &CODECAPI_AVLowLatencyMode, 1);
    let _ = set_codec_u32(
        api,
        &CODECAPI_AVEncCommonRateControlMode,
        eAVEncCommonRateControlMode_CBR.0 as u32,
    );
    let _ = set_codec_u32(api, &CODECAPI_AVEncCommonMeanBitRate, bitrate_bps);
    // 0 = fastest/worst, 100 = slowest/best. Mid-high keeps text readable without
    // a big latency cliff on NVENC/QuickSync.
    let _ = set_codec_u32(api, &CODECAPI_AVEncCommonQualityVsSpeed, 60);
}

unsafe fn set_codec_u32(api: &ICodecAPI, key: &GUID, value: u32) -> Result<()> {
    // VARIANT's fields are ManuallyDrop unions; write through the deref
    // explicitly rather than assigning, which would run a destructor on
    // uninitialised memory.
    let mut v = VARIANT::default();
    let inner = &mut *v.Anonymous.Anonymous;
    inner.vt = VT_UI4;
    inner.Anonymous.ulVal = value;
    api.SetValue(key, &v)?;
    Ok(())
}

/// MF packs paired 32-bit values (size, frame rate, aspect) into one 64-bit attribute.
unsafe fn set_ratio(ty: &IMFMediaType, key: &GUID, high: u32, low: u32) -> Result<()> {
    ty.SetUINT64(key, ((high as u64) << 32) | low as u64)?;
    Ok(())
}

unsafe fn read_sequence_header(transform: &IMFTransform) -> Vec<u8> {
    let Ok(ty) = transform.GetOutputCurrentType(0) else {
        return Vec::new();
    };
    let Ok(size) = ty.GetBlobSize(&MF_MT_MPEG_SEQUENCE_HEADER) else {
        return Vec::new();
    };
    if size == 0 {
        return Vec::new();
    }
    let mut blob = vec![0u8; size as usize];
    let mut written = 0u32;
    if ty
        .GetBlob(&MF_MT_MPEG_SEQUENCE_HEADER, &mut blob, Some(&mut written))
        .is_err()
    {
        return Vec::new();
    }
    blob.truncate(written as usize);
    // Only usable if it is already Annex-B. Anything else is a configuration record
    // in disguise and must not be spliced into the byte stream.
    if !blob.starts_with(&START_CODE) && !blob.starts_with(&START_CODE[1..]) {
        tracing::warn!(
            "sequence header is not Annex-B ({} bytes) — ignoring, will learn from the stream",
            blob.len()
        );
        return Vec::new();
    }
    tracing::info!("sequence header: {} bytes of Annex-B SPS/PPS", blob.len());
    blob
}

/// Does this access unit already carry an SPS (NAL type 7)?
fn contains_sps(data: &[u8]) -> bool {
    nal_types(data).any(|t| t == 7)
}

/// Pull the SPS (7) and PPS (8) NALs out of an access unit, start codes included,
/// so they can be replayed ahead of a later keyframe for a viewer that joined after
/// the encoder emitted them.
fn extract_parameter_sets(data: &[u8]) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    let mut have_sps = false;
    let mut have_pps = false;
    for (start, end) in nal_ranges(data) {
        let nal_type = data[start] & 0x1F;
        if nal_type != 7 && nal_type != 8 {
            continue;
        }
        out.extend_from_slice(&START_CODE);
        out.extend_from_slice(&data[start..end]);
        have_sps |= nal_type == 7;
        have_pps |= nal_type == 8;
    }
    (have_sps && have_pps).then_some(out)
}

/// Byte ranges of each NAL payload (after its start code).
fn nal_ranges(data: &[u8]) -> Vec<(usize, usize)> {
    let mut starts = Vec::new();
    let mut i = 0usize;
    while i + 3 <= data.len() {
        if data[i] == 0 && data[i + 1] == 0 && data[i + 2] == 1 {
            starts.push(i + 3);
            i += 3;
        } else if i + 4 <= data.len()
            && data[i] == 0
            && data[i + 1] == 0
            && data[i + 2] == 0
            && data[i + 3] == 1
        {
            starts.push(i + 4);
            i += 4;
        } else {
            i += 1;
        }
    }
    let mut ranges = Vec::with_capacity(starts.len());
    for (n, &start) in starts.iter().enumerate() {
        // A NAL ends where the next one's start code begins.
        let end = starts
            .get(n + 1)
            .map(|&next| {
                let mut e = next.saturating_sub(3);
                if e >= 1 && data.get(e - 1) == Some(&0) {
                    e -= 1;
                }
                e
            })
            .unwrap_or(data.len());
        if start < end {
            ranges.push((start, end));
        }
    }
    ranges
}

fn nal_types(data: &[u8]) -> impl Iterator<Item = u8> + '_ {
    let mut i = 0usize;
    std::iter::from_fn(move || {
        while i + 3 < data.len() {
            let at_start = data[i] == 0 && data[i + 1] == 0 && data[i + 2] == 1;
            let at_long = i + 4 < data.len()
                && data[i] == 0
                && data[i + 1] == 0
                && data[i + 2] == 0
                && data[i + 3] == 1;
            if at_long {
                let t = data[i + 4] & 0x1F;
                i += 5;
                return Some(t);
            }
            if at_start {
                let t = data[i + 3] & 0x1F;
                i += 4;
                return Some(t);
            }
            i += 1;
        }
        None
    })
}

/// AVCC (4-byte big-endian length prefixes) to Annex-B start codes.
fn avcc_to_annex_b(data: &[u8]) -> Result<Vec<u8>> {
    let mut out = Vec::with_capacity(data.len() + 16);
    let mut i = 0usize;
    while i + 4 <= data.len() {
        let len = u32::from_be_bytes([data[i], data[i + 1], data[i + 2], data[i + 3]]) as usize;
        i += 4;
        if len == 0 || i + len > data.len() {
            bail!("malformed AVCC unit: length {len} at offset {i} of {}", data.len());
        }
        out.extend_from_slice(&START_CODE);
        out.extend_from_slice(&data[i..i + len]);
        i += len;
    }
    Ok(out)
}

const fn nv12_len(width: u32, height: u32) -> usize {
    (width as usize * height as usize) + (width as usize * height as usize / 2)
}

/// BGRA -> NV12 (BT.601), the input format hardware encoders universally accept.
/// Chroma is interleaved (UVUV) and half resolution in both axes.
pub fn bgra_to_nv12(bgra: &[u8], width: usize, height: usize, out: &mut Vec<u8>) {
    let y_size = width * height;
    out.resize(y_size + y_size / 2, 0);
    let (y_plane, uv_plane) = out.split_at_mut(y_size);
    let stride = width * 4;

    for y in 0..height {
        let Some(row) = bgra.get(y * stride..y * stride + stride) else {
            break;
        };
        let y_row = &mut y_plane[y * width..(y + 1) * width];
        for (px, out) in row.chunks_exact(4).zip(y_row.iter_mut()) {
            let b = px[0] as i32;
            let g = px[1] as i32;
            let r = px[2] as i32;
            *out = ((((66 * r + 129 * g + 25 * b + 128) >> 8) + 16).clamp(0, 255)) as u8;
        }
        if y % 2 != 0 {
            continue;
        }
        let uv_row = &mut uv_plane[(y / 2) * width..(y / 2) * width + width];
        for (cx, uv) in uv_row.chunks_exact_mut(2).enumerate() {
            let px = &row[cx * 8..cx * 8 + 4];
            let b = px[0] as i32;
            let g = px[1] as i32;
            let r = px[2] as i32;
            uv[0] = ((((-38 * r - 74 * g + 112 * b + 128) >> 8) + 128).clamp(0, 255)) as u8;
            uv[1] = ((((112 * r - 94 * g - 18 * b + 128) >> 8) + 128).clamp(0, 255)) as u8;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn avcc_converts_to_annex_b() {
        // Two units: [0x65 0xAA] and [0x41].
        let avcc = [0, 0, 0, 2, 0x65, 0xAA, 0, 0, 0, 1, 0x41];
        let out = avcc_to_annex_b(&avcc).unwrap();
        assert_eq!(out, vec![0, 0, 0, 1, 0x65, 0xAA, 0, 0, 0, 1, 0x41]);
    }

    #[test]
    fn malformed_avcc_is_an_error_not_a_panic() {
        let avcc = [0, 0, 0, 99, 0x65];
        assert!(avcc_to_annex_b(&avcc).is_err());
    }

    #[test]
    fn parameter_sets_are_extracted_for_replay() {
        // SPS (7), PPS (8), then an IDR (5).
        let au = [
            0, 0, 0, 1, 0x67, 0x42, 0xE0, //
            0, 0, 0, 1, 0x68, 0xCE, //
            0, 0, 0, 1, 0x65, 0xAA, 0xBB,
        ];
        let sets = extract_parameter_sets(&au).expect("sps+pps found");
        assert!(contains_sps(&sets));
        assert!(nal_types(&sets).any(|t| t == 8), "pps must be included");
        assert!(!nal_types(&sets).any(|t| t == 5), "slice data must not be");
    }

    #[test]
    fn parameter_sets_need_both_sps_and_pps() {
        let sps_only = [0, 0, 0, 1, 0x67, 0x42, 0, 0, 0, 1, 0x65, 0xAA];
        assert!(extract_parameter_sets(&sps_only).is_none());
    }

    #[test]
    fn sps_detection_finds_type_7() {
        let with_sps = [0, 0, 0, 1, 0x67, 0x42, 0, 0, 0, 1, 0x65];
        let without = [0, 0, 0, 1, 0x65, 0xAA];
        assert!(contains_sps(&with_sps));
        assert!(!contains_sps(&without));
    }

    #[test]
    fn nv12_is_the_right_size_and_luma_matches_bt601() {
        let white = vec![255u8; 4 * 4 * 4];
        let mut out = Vec::new();
        bgra_to_nv12(&white, 4, 4, &mut out);
        assert_eq!(out.len(), nv12_len(4, 4));
        assert_eq!(out[0], 235, "white luma is 235 in studio swing");
        let black = vec![0u8; 4 * 4 * 4];
        bgra_to_nv12(&black, 4, 4, &mut out);
        assert_eq!(out[0], 16, "black luma is 16 in studio swing");
        // Neutral chroma for greyscale input.
        assert_eq!(out[16], 128);
        assert_eq!(out[17], 128);
    }

    #[test]
    fn a_short_source_does_not_panic() {
        let short = vec![0u8; 4 * 2 * 4];
        let mut out = Vec::new();
        bgra_to_nv12(&short, 4, 4, &mut out);
        assert_eq!(out.len(), nv12_len(4, 4));
    }
}
