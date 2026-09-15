//! WASAPI loopback capture → Opus encode → `AudioFrame`, Windows-only.
//!
//! Two capture modes:
//!
//! - Process-scoped (`spawn_wasapi_process_loopback_capture`): captures only
//!   the audio rendered by one process (and its child processes), via the
//!   Windows 10 2004+ "process loopback" activation. Used whenever win-capture
//!   is following a specific window (`--source window`), so a friend only
//!   hears the game — not Discord notifications, browser tabs, or anything
//!   else on the host's speakers.
//! - System-wide (`spawn_wasapi_loopback_capture`): the original default-
//!   render-endpoint loopback, used for desktop/picker capture where there is
//!   no single target process to scope to.
//!
//! Fail-open by design: if any COM/WASAPI call fails (no default device, a
//! locked-down audio session, process loopback unsupported on this Windows
//! build, etc.) this logs a warning and the capture stream stays video-only —
//! matching how the rest of win-capture treats a missing/optional subsystem.

use crate::audio::AudioFrame;
use anyhow::{bail, Context, Result};
use std::sync::mpsc::SyncSender;
use std::time::Duration;
use windows::core::{implement, Interface, PCWSTR};
use windows::Win32::Foundation::{HANDLE, WAIT_OBJECT_0};
use windows::Win32::Media::Audio::{
    eConsole, eRender, ActivateAudioInterfaceAsync, IActivateAudioInterfaceAsyncOperation,
    IActivateAudioInterfaceCompletionHandler, IActivateAudioInterfaceCompletionHandler_Impl,
    IAudioCaptureClient, IAudioClient, IMMDeviceEnumerator, MMDeviceEnumerator,
    AUDCLNT_BUFFERFLAGS_SILENT, AUDCLNT_SHAREMODE_SHARED, AUDCLNT_STREAMFLAGS_LOOPBACK,
    AUDIOCLIENT_ACTIVATION_PARAMS, AUDIOCLIENT_ACTIVATION_PARAMS_0,
    AUDIOCLIENT_ACTIVATION_TYPE_PROCESS_LOOPBACK, AUDIOCLIENT_PROCESS_LOOPBACK_PARAMS,
    PROCESS_LOOPBACK_MODE_INCLUDE_TARGET_PROCESS_TREE, WAVE_FORMAT_PCM,
};
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CLSCTX_ALL, COINIT_MULTITHREADED,
};
use windows::Win32::System::Threading::{CreateEventW, SetEvent, WaitForSingleObject, INFINITE};

/// Opus wants 2.5/5/10/20/40/60ms frames — 20ms is the same cadence the
/// native client's playback side (`crates/client/src/audio.rs`) already
/// assumes (`FRAMES_PER_PACKET = 960`).
const OPUS_FRAME_SAMPLES: usize = 960;
const TARGET_RATE: u32 = 48_000;
const TARGET_CHANNELS: usize = 2;
/// 1s WASAPI ring buffer — generous; loopback capture only needs to drain
/// faster than the device fills it, this is not the end-to-end latency.
const BUFFER_DURATION_100NS: i64 = 10_000_000;

/// Spawn the loopback-capture thread. Never blocks the caller — failures are
/// logged and the thread exits, leaving video capture completely unaffected.
///
/// Captures the *entire* default output device — every app on the host, not
/// just one game. Only appropriate when there is no single target process to
/// scope to (desktop / picker capture). Window-source capture must use
/// [`spawn_wasapi_process_loopback_capture`] instead.
pub fn spawn_wasapi_loopback_capture(tx: SyncSender<AudioFrame>) {
    std::thread::spawn(move || {
        if let Err(e) = run(tx) {
            tracing::warn!(
                "WASAPI loopback audio capture unavailable ({e:#}) — continuing video-only"
            );
        }
    });
}

/// Spawn a process-scoped loopback-capture thread: only audio rendered by
/// `target_pid` (and its child processes — an emulator's game process is
/// commonly a child of the emulator's own PID) crosses the wire, via the
/// Windows 10 2004+ "process loopback" activation. This is what keeps a
/// friend from hearing Discord pings, browser tabs, or anything else on the
/// host's speakers while a specific game window is being streamed.
///
/// Falls back to the full system-loopback capture if process loopback fails
/// to activate — e.g. an older Windows build that predates the API — so
/// audio degrades to "everything" rather than disappearing entirely.
pub fn spawn_wasapi_process_loopback_capture(tx: SyncSender<AudioFrame>, target_pid: u32) {
    std::thread::spawn(move || {
        if let Err(e) = run_process(tx.clone(), target_pid) {
            tracing::warn!(
                "process-scoped WASAPI loopback unavailable for pid {target_pid} ({e:#}) — \
                 falling back to whole-system audio"
            );
            if let Err(e) = run(tx) {
                tracing::warn!(
                    "WASAPI loopback audio capture unavailable ({e:#}) — continuing video-only"
                );
            }
        }
    });
}

fn run(tx: SyncSender<AudioFrame>) -> Result<()> {
    // COM apartments are per-thread — this thread needs its own init even
    // though the video capture thread already called it for itself.
    unsafe {
        CoInitializeEx(None, COINIT_MULTITHREADED)
            .ok()
            .context("CoInitializeEx (audio thread)")?;

        let enumerator: IMMDeviceEnumerator =
            CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)
                .context("CoCreateInstance(MMDeviceEnumerator)")?;
        let device = enumerator
            .GetDefaultAudioEndpoint(eRender, eConsole)
            .context("GetDefaultAudioEndpoint")?;
        let client: IAudioClient = device
            .Activate::<IAudioClient>(CLSCTX_ALL, None)
            .context("Activate IAudioClient")?;

        let wfx = client.GetMixFormat().context("GetMixFormat")?;

        client
            .Initialize(
                AUDCLNT_SHAREMODE_SHARED,
                AUDCLNT_STREAMFLAGS_LOOPBACK,
                BUFFER_DURATION_100NS,
                0,
                wfx,
                None,
            )
            .context("IAudioClient::Initialize (loopback)")?;

        capture_loop(&client, wfx, tx)
    }
}

/// COM completion handler for `ActivateAudioInterfaceAsync` — the process-
/// loopback activation is asynchronous even though every other WASAPI call
/// used here is synchronous, so this just signals an event the calling
/// thread is blocked on.
#[implement(IActivateAudioInterfaceCompletionHandler)]
struct ActivateHandler {
    event: HANDLE,
}

impl IActivateAudioInterfaceCompletionHandler_Impl for ActivateHandler_Impl {
    fn ActivateCompleted(
        &self,
        _operation: windows::core::Ref<'_, IActivateAudioInterfaceAsyncOperation>,
    ) -> windows::core::Result<()> {
        unsafe { SetEvent(self.event) }?;
        Ok(())
    }
}

fn run_process(tx: SyncSender<AudioFrame>, target_pid: u32) -> Result<()> {
    unsafe {
        CoInitializeEx(None, COINIT_MULTITHREADED)
            .ok()
            .context("CoInitializeEx (process audio thread)")?;

        let mut params = AUDIOCLIENT_ACTIVATION_PARAMS {
            ActivationType: AUDIOCLIENT_ACTIVATION_TYPE_PROCESS_LOOPBACK,
            Anonymous: AUDIOCLIENT_ACTIVATION_PARAMS_0 {
                ProcessLoopbackParams: AUDIOCLIENT_PROCESS_LOOPBACK_PARAMS {
                    TargetProcessId: target_pid,
                    ProcessLoopbackMode: PROCESS_LOOPBACK_MODE_INCLUDE_TARGET_PROCESS_TREE,
                },
            },
        };
        let params_blob = std::slice::from_raw_parts(
            &mut params as *mut _ as *mut u8,
            std::mem::size_of::<AUDIOCLIENT_ACTIVATION_PARAMS>(),
        );
        let mut prop_variant = build_activation_propvariant(params_blob)?;

        let event = CreateEventW(None, true, false, None).context("CreateEventW")?;
        let handler: IActivateAudioInterfaceCompletionHandler =
            ActivateHandler { event }.into();

        // The device id for process-loopback activation isn't a real endpoint
        // id — it's this fixed virtual-device string mmdeviceapi.h defines.
        let device_id: Vec<u16> = "VAD\\Process_Loopback\0".encode_utf16().collect();
        let op: IActivateAudioInterfaceAsyncOperation = ActivateAudioInterfaceAsync(
            PCWSTR(device_id.as_ptr()),
            &IAudioClient::IID,
            Some(&mut prop_variant),
            &handler,
        )
        .context("ActivateAudioInterfaceAsync")?;

        if WaitForSingleObject(event, INFINITE) != WAIT_OBJECT_0 {
            bail!("WaitForSingleObject on activation event failed");
        }

        let mut activate_result = windows::core::HRESULT(0);
        let mut audio_client_unknown: Option<windows::core::IUnknown> = None;
        op.GetActivateResult(&mut activate_result, &mut audio_client_unknown)
            .context("GetActivateResult")?;
        activate_result.ok().context("process loopback activation")?;
        let client: IAudioClient = audio_client_unknown
            .context("activation returned no interface")?
            .cast()
            .context("cast activated interface to IAudioClient")?;

        // Process loopback only ever delivers 32-bit float, 2ch, 48kHz — it
        // has no "mix format" of its own to query, unlike a real endpoint.
        let mut wfx = windows::Win32::Media::Audio::WAVEFORMATEX {
            wFormatTag: windows::Win32::Media::Multimedia::WAVE_FORMAT_IEEE_FLOAT as u16,
            nChannels: TARGET_CHANNELS as u16,
            nSamplesPerSec: TARGET_RATE,
            nAvgBytesPerSec: TARGET_RATE * TARGET_CHANNELS as u32 * 4,
            nBlockAlign: (TARGET_CHANNELS * 4) as u16,
            wBitsPerSample: 32,
            cbSize: 0,
        };

        client
            .Initialize(
                AUDCLNT_SHAREMODE_SHARED,
                AUDCLNT_STREAMFLAGS_LOOPBACK,
                BUFFER_DURATION_100NS,
                0,
                &mut wfx,
                None,
            )
            .context("IAudioClient::Initialize (process loopback)")?;

        capture_loop(&client, &mut wfx, tx)
    }
}

/// Wraps a raw `AUDIOCLIENT_ACTIVATION_PARAMS` blob in the blob-typed
/// `PROPVARIANT` `ActivateAudioInterfaceAsync` expects as its activation
/// params argument.
unsafe fn build_activation_propvariant(
    blob: &[u8],
) -> Result<windows::Win32::System::Com::StructuredStorage::PROPVARIANT> {
    use windows::Win32::System::Com::{StructuredStorage::PROPVARIANT, BLOB};
    use windows::Win32::System::Variant::VT_BLOB;

    // The blob must outlive the activation call; leak it deliberately — this
    // runs once per capture-thread lifetime, not in a hot loop.
    let leaked: &'static [u8] = Box::leak(blob.to_vec().into_boxed_slice());
    let mut pv: PROPVARIANT = std::mem::zeroed();
    let inner = &mut pv.Anonymous.Anonymous;
    inner.vt = VT_BLOB;
    inner.Anonymous.blob = BLOB {
        cbSize: leaked.len() as u32,
        pBlobData: leaked.as_ptr() as *mut u8,
    };
    Ok(pv)
}

unsafe fn capture_loop(
    client: &IAudioClient,
    wfx: *mut windows::Win32::Media::Audio::WAVEFORMATEX,
    tx: SyncSender<AudioFrame>,
) -> Result<()> {
    let src_rate = (*wfx).nSamplesPerSec;
    let src_channels = (*wfx).nChannels as usize;
    // Modern Windows mix formats are IEEE float (tag 3) or EXTENSIBLE
    // wrapping float (tag 0xFFFE) essentially universally; true 16-bit
    // PCM mix formats are a legacy case this still handles correctly.
    let is_float = (*wfx).wFormatTag != WAVE_FORMAT_PCM as u16;

    let capture_client: IAudioCaptureClient =
        client.GetService().context("GetService(IAudioCaptureClient)")?;
    client.Start().context("IAudioClient::Start")?;

    {
        let mut encoder = opus::Encoder::new(
            TARGET_RATE,
            opus::Channels::Stereo,
            opus::Application::Audio,
        )
        .context("opus::Encoder::new")?;

        // Interleaved stereo f32 @ 48kHz, resampled/downmixed from whatever
        // the device's mix format actually is.
        let mut pcm_queue: Vec<f32> = Vec::with_capacity(OPUS_FRAME_SAMPLES * TARGET_CHANNELS * 4);
        let mut seq: u32 = 0;
        let mut encode_buf = vec![0u8; 4000];

        loop {
            std::thread::sleep(Duration::from_millis(10));
            loop {
                let packet_len = capture_client.GetNextPacketSize().context("GetNextPacketSize")?;
                if packet_len == 0 {
                    break;
                }
                let mut data_ptr = std::ptr::null_mut();
                let mut num_frames = 0u32;
                let mut flags = 0u32;
                capture_client
                    .GetBuffer(&mut data_ptr, &mut num_frames, &mut flags, None, None)
                    .context("GetBuffer")?;

                if num_frames > 0 {
                    let silent = flags & AUDCLNT_BUFFERFLAGS_SILENT.0 as u32 != 0;
                    let n_samples = num_frames as usize * src_channels;
                    let samples: Vec<f32> = if silent || data_ptr.is_null() {
                        vec![0.0; n_samples]
                    } else if is_float {
                        std::slice::from_raw_parts(data_ptr as *const f32, n_samples).to_vec()
                    } else {
                        std::slice::from_raw_parts(data_ptr as *const i16, n_samples)
                            .iter()
                            .map(|&v| v as f32 / 32768.0)
                            .collect()
                    };
                    append_resampled_stereo(&mut pcm_queue, &samples, src_channels, src_rate);
                }

                capture_client.ReleaseBuffer(num_frames).context("ReleaseBuffer")?;

                let frame_len = OPUS_FRAME_SAMPLES * TARGET_CHANNELS;
                while pcm_queue.len() >= frame_len {
                    let chunk: Vec<f32> = pcm_queue.drain(0..frame_len).collect();
                    match encoder.encode_float(&chunk, &mut encode_buf) {
                        Ok(n) => {
                            seq = seq.wrapping_add(1);
                            let frame = AudioFrame {
                                seq,
                                sample_rate: TARGET_RATE,
                                channels: TARGET_CHANNELS as u8,
                                opus: encode_buf[..n].to_vec(),
                            };
                            // Drop rather than block — a full queue means the
                            // writer thread is behind; audio must never stall
                            // capture the way a dropped video frame must not.
                            let _ = tx.try_send(frame);
                        }
                        Err(e) => tracing::warn!("opus encode failed: {e}"),
                    }
                }
            }
        }
    }
}

/// Downmix/upmix each frame to stereo, then (if needed) resample to 48kHz
/// with simple linear interpolation. Not audiophile-grade, but more than
/// sufficient for game audio/voice over a lossy Opus link at this bitrate.
fn append_resampled_stereo(queue: &mut Vec<f32>, samples: &[f32], src_channels: usize, src_rate: u32) {
    if src_channels == 0 || samples.is_empty() {
        return;
    }
    let n_frames = samples.len() / src_channels;
    let mut stereo: Vec<f32> = Vec::with_capacity(n_frames * 2);
    for f in 0..n_frames {
        let base = f * src_channels;
        let (l, r) = match src_channels {
            1 => (samples[base], samples[base]),
            2 => (samples[base], samples[base + 1]),
            n => {
                let sum: f32 = samples[base..base + n].iter().sum();
                let m = sum / n as f32;
                (m, m)
            }
        };
        stereo.push(l);
        stereo.push(r);
    }

    if src_rate == TARGET_RATE {
        queue.extend_from_slice(&stereo);
        return;
    }

    let src_frames = stereo.len() / 2;
    if src_frames < 2 {
        return;
    }
    let ratio = TARGET_RATE as f64 / src_rate as f64;
    let out_frames = ((src_frames as f64) * ratio) as usize;
    for i in 0..out_frames {
        let src_pos = i as f64 / ratio;
        let idx = (src_pos.floor() as usize).min(src_frames - 1);
        let idx2 = (idx + 1).min(src_frames - 1);
        let frac = (src_pos - idx as f64) as f32;
        for ch in 0..2 {
            let a = stereo[idx * 2 + ch];
            let b = stereo[idx2 * 2 + ch];
            queue.push(a + (b - a) * frac);
        }
    }
}
