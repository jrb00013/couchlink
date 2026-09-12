//! WASAPI loopback capture → Opus encode → `AudioFrame`, Windows-only.
//!
//! Captures whatever the default render (speaker) device is playing —
//! game audio included — via WASAPI's loopback flag on that render endpoint.
//! Independent of which window video capture is following: this always
//! follows the system's default output device, not any one process.
//!
//! Fail-open by design: if any COM/WASAPI call fails (no default device, a
//! locked-down audio session, etc.) this logs a warning and the capture
//! stream stays video-only — matching how the rest of win-capture treats a
//! missing/optional subsystem.

use crate::audio::AudioFrame;
use anyhow::{Context, Result};
use std::sync::mpsc::SyncSender;
use std::time::Duration;
use windows::Win32::Media::Audio::{
    eConsole, eRender, IAudioCaptureClient, IAudioClient, IMMDeviceEnumerator,
    MMDeviceEnumerator, AUDCLNT_BUFFERFLAGS_SILENT, AUDCLNT_SHAREMODE_SHARED,
    AUDCLNT_STREAMFLAGS_LOOPBACK, WAVE_FORMAT_PCM,
};
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CLSCTX_ALL, COINIT_MULTITHREADED,
};

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
pub fn spawn_wasapi_loopback_capture(tx: SyncSender<AudioFrame>) {
    std::thread::spawn(move || {
        if let Err(e) = run(tx) {
            tracing::warn!(
                "WASAPI loopback audio capture unavailable ({e:#}) — continuing video-only"
            );
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
        let src_rate = (*wfx).nSamplesPerSec;
        let src_channels = (*wfx).nChannels as usize;
        // Modern Windows mix formats are IEEE float (tag 3) or EXTENSIBLE
        // wrapping float (tag 0xFFFE) essentially universally; true 16-bit
        // PCM mix formats are a legacy case this still handles correctly.
        let is_float = (*wfx).wFormatTag != WAVE_FORMAT_PCM as u16;

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
        let capture_client: IAudioCaptureClient =
            client.GetService().context("GetService(IAudioCaptureClient)")?;
        client.Start().context("IAudioClient::Start")?;

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
