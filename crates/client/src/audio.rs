//! Native audio pipe — Opus RTP → cpal, separate from video decode.
//! Never touches video backlog, PLI, or governor. One decode+playback per audio track.
//! Fail-open: if cpal or opus is unavailable, audio is dropped and video continues.

use anyhow::Result;
use tracing::{info, warn};

const SAMPLE_RATE: u32 = 48000;
const CHANNELS: u16 = 2;
const FRAMES_PER_PACKET: usize = 960;

pub struct AudioOutput {
    sender: Option<tokio::sync::mpsc::UnboundedSender<Vec<u8>>>,
}

impl AudioOutput {
    pub fn sender(&self) -> Option<tokio::sync::mpsc::UnboundedSender<Vec<u8>>> {
        self.sender.clone()
    }
    #[allow(dead_code)]
    pub fn push_opus(&self, data: Vec<u8>) {
        if let Some(tx) = &self.sender {
            let _ = tx.send(data);
        }
    }
}

/// Spawn audio output. With `audio` feature: cpal+opus; without: no-op sink that still drains RTP.
pub fn spawn_audio_output() -> Option<AudioOutput> {
    if std::env::var("COUCHLINK_AUDIO").as_deref() == Ok("0") {
        info!("audio disabled via COUCHLINK_AUDIO=0");
        return None;
    }
    match try_spawn() {
        Ok(o) => {
            info!("native audio output ready (48kHz stereo, separate RTP pipe)");
            Some(o)
        }
        Err(e) => {
            warn!("native audio unavailable ({e:#}) — continuing video-only");
            None
        }
    }
}

#[cfg(feature = "audio")]
fn try_spawn() -> Result<AudioOutput> {
    use std::sync::{Arc, Mutex};
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<Vec<u8>>();
    std::thread::Builder::new()
        .name("couchlink-audio".into())
        .spawn(move || {
            if let Err(e) = run_cpal_loop(&mut rx) {
                warn!("audio thread exited ({e:#}) — draining opus silently");
                while rx.blocking_recv().is_some() {}
            }
        })
        .map_err(|e| anyhow::anyhow!("spawn audio thread: {e}"))?;
    Ok(AudioOutput { sender: Some(tx) })
}

#[cfg(feature = "audio")]
fn run_cpal_loop(rx: &mut tokio::sync::mpsc::UnboundedReceiver<Vec<u8>>) -> Result<()> {
    use std::sync::{Arc, Mutex};
    let host = cpal::host_from_id(cpal::HostId::Alsa)
        .or_else(|_| cpal::host_from_id(cpal::HostId::Pulse))
        .or_else(|_| Ok::<_, anyhow::Error>(cpal::default_host()))
        .map_err(|e| anyhow::anyhow!("cpal host: {e}"))?;
    let device = host.default_output_device().ok_or_else(|| anyhow::anyhow!("no audio device"))?;
    let config = device.default_output_config().map_err(|e| anyhow::anyhow!("config: {e}"))?;
    let ring: Arc<Mutex<Vec<f32>>> = Arc::new(Mutex::new(Vec::with_capacity(48000 * 2)));
    let ring_cb = Arc::clone(&ring);
    let opus_decoder = opus::Decoder::new(SAMPLE_RATE, opus::Channels::Stereo).map_err(|e| anyhow::anyhow!("opus: {e}"))?;
    let decoder = Arc::new(Mutex::new(opus_decoder));
    let stream_config: cpal::StreamConfig = config.into();
    let channels = stream_config.channels as usize;
    let stream = device
        .build_output_stream(
            &stream_config,
            move |data: &mut [f32], _: &cpal::OutputCallbackInfo| {
                let mut buf = ring_cb.lock().unwrap();
                let needed = data.len();
                if buf.len() >= needed {
                    data.copy_from_slice(&buf[..needed]);
                    buf.drain(..needed);
                } else {
                    let avail = buf.len();
                    if avail > 0 {
                        data[..avail].copy_from_slice(&buf[..]);
                        buf.clear();
                    }
                    for v in &mut data[avail..] {
                        *v = 0.0;
                    }
                }
                if buf.len() > (SAMPLE_RATE as usize * channels / 5) {
                    let excess = buf.len() - (SAMPLE_RATE as usize * channels / 10);
                    buf.drain(..excess);
                }
            },
            |e| warn!("cpal error: {e}"),
            None,
        )
        .map_err(|e| anyhow::anyhow!("build_output_stream: {e}"))?;
    use cpal::traits::StreamTrait;
    stream.play().map_err(|e| anyhow::anyhow!("play: {e}"))?;
    let _keep = Box::new(stream) as Box<dyn Send>;
    let keep_ptr = Box::into_raw(_keep) as *mut Box<dyn Send>;
    let _guard = scopeguard::guard(keep_ptr, |p| unsafe { let _ = Box::from_raw(p); });
    while let Some(opus) = rx.blocking_recv() {
        let mut dec = decoder.lock().unwrap();
        let mut pcm = vec![0f32; FRAMES_PER_PACKET * CHANNELS as usize];
        let decoded = match dec.decode_float(&opus, &mut pcm, false) {
            Ok(n) => n,
            Err(e) => { warn!("opus decode: {e}"); continue; }
        };
        let samples = decoded * CHANNELS as usize;
        pcm.truncate(samples);
        let mut buf = ring.lock().unwrap();
        buf.extend_from_slice(&pcm);
        let max = SAMPLE_RATE as usize * CHANNELS as usize / 8;
        if buf.len() > max { let drop = buf.len() - max; buf.drain(..drop); }
    }
    Ok(())
}

#[cfg(not(feature = "audio"))]
fn try_spawn() -> Result<AudioOutput> {
    // No opus/cpal — stub sink that drains RTP so SRTP doesn't backpressure, but no audible output.
    // Still proves the separate pipe is consumed and never blocks video.
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<Vec<u8>>();
    std::thread::Builder::new()
        .name("couchlink-audio-stub".into())
        .spawn(move || {
            info!("audio stub draining (build with --features audio for audible output)");
            while rx.blocking_recv().is_some() {}
        })
        .map_err(|e| anyhow::anyhow!("spawn stub: {e}"))?;
    Ok(AudioOutput { sender: Some(tx) })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn spawn_or_degrade_does_not_panic() {
        let out = spawn_audio_output();
        let _ = out;
    }
    #[test]
    fn constants_match_opus_20ms() {
        assert_eq!(SAMPLE_RATE, 48000);
        assert_eq!(CHANNELS, 2);
        assert_eq!(FRAMES_PER_PACKET, 960);
    }
    #[test]
    fn audio_push_does_not_block_video() {
        let out = try_spawn().unwrap();
        let start = std::time::Instant::now();
        for _ in 0..100 {
            out.push_opus(vec![0u8; 100]);
        }
        assert!(start.elapsed().as_millis() < 50, "audio push must be non-blocking");
    }
}
