//! Audio pipe for the capture socket — `CLA1` frames carrying Opus.
//!
//! Format is deliberately tiny and separate from `CLF2` video frames so the
//! reader can distinguish by 4-byte magic and never over-read. Each frame is
//! `CLA1` + `seq:u32 LE` + `sample_rate:u32 LE` + `channels:u8` + `opus_len:u32 LE` + `opus`.
//! All reads slice **to length**, never `buf[offset..]` remainder — same fix as the
//! SCTP vendor patch. A truncated `CLA1` returns error and leaves the next `CLF2` intact
//! when parsed from a buffered stream via `try_read_audio_frame`.

use anyhow::{bail, Context, Result};
use std::io::{Read, Write};

pub const AUDIO_MAGIC: &[u8; 4] = b"CLA1";
const MAX_OPUS: usize = 64 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AudioFrame {
    pub seq: u32,
    pub sample_rate: u32,
    pub channels: u8,
    pub opus: Vec<u8>,
}

pub fn write_audio_frame(w: &mut impl Write, frame: &AudioFrame) -> Result<()> {
    if frame.opus.len() > MAX_OPUS {
        bail!("opus payload too large {}", frame.opus.len());
    }
    w.write_all(AUDIO_MAGIC)?;
    w.write_all(&frame.seq.to_le_bytes())?;
    w.write_all(&frame.sample_rate.to_le_bytes())?;
    w.write_all(&[frame.channels])?;
    w.write_all(&(frame.opus.len() as u32).to_le_bytes())?;
    w.write_all(&frame.opus)?;
    w.flush()?;
    Ok(())
}

pub fn read_audio_frame(r: &mut impl Read) -> Result<AudioFrame> {
    let mut magic = [0u8; 4];
    r.read_exact(&mut magic).context("audio magic")?;
    if &magic != AUDIO_MAGIC {
        bail!("bad audio magic {magic:?}");
    }
    read_audio_body(r)
}

fn read_audio_body(r: &mut impl Read) -> Result<AudioFrame> {
    let mut head = [0u8; 13];
    r.read_exact(&mut head).context("audio header")?;
    let seq = u32::from_le_bytes(head[0..4].try_into()?);
    let sample_rate = u32::from_le_bytes(head[4..8].try_into()?);
    let channels = head[8];
    let len = u32::from_le_bytes(head[9..13].try_into()?) as usize;
    if len > MAX_OPUS {
        bail!("invalid opus size {len}");
    }
    let mut opus = vec![0u8; len];
    if len > 0 {
        r.read_exact(&mut opus).context("opus payload")?;
    }
    Ok(AudioFrame {
        seq,
        sample_rate,
        channels,
        opus,
    })
}

/// Try to parse one `CLA1` frame from a buffered slice without consuming on short.
/// Returns `Ok(None)` if magic is not `CLA1` (caller should try `CLF2`).
/// Returns `Err` on truncated but magic-matched data — caller must not skip bytes.
pub fn try_read_audio_frame(buf: &[u8]) -> Result<Option<(AudioFrame, usize)>> {
    if buf.len() < 4 {
        return Ok(None);
    }
    if &buf[0..4] != AUDIO_MAGIC {
        return Ok(None);
    }
    if buf.len() < 17 {
        bail!("truncated CLA1 header");
    }
    let len = u32::from_le_bytes(buf[13..17].try_into()?) as usize;
    if len > MAX_OPUS {
        bail!("invalid opus size {len}");
    }
    let total = 17 + len;
    if buf.len() < total {
        bail!("truncated CLA1 payload");
    }
    let seq = u32::from_le_bytes(buf[4..8].try_into()?);
    let sample_rate = u32::from_le_bytes(buf[8..12].try_into()?);
    let channels = buf[12];
    let opus = buf[17..total].to_vec();
    Ok(Some((
        AudioFrame {
            seq,
            sample_rate,
            channels,
            opus,
        },
        total,
    )))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn roundtrip() {
        let f = AudioFrame {
            seq: 42,
            sample_rate: 48000,
            channels: 2,
            opus: vec![1, 2, 3, 4, 5],
        };
        let mut wire = Vec::new();
        write_audio_frame(&mut wire, &f).unwrap();
        let back = read_audio_frame(&mut Cursor::new(&wire)).unwrap();
        assert_eq!(back, f);
    }

    #[test]
    fn clf2_then_cla1_round_trip_without_eating_the_next_frame() {
        use crate::{write_frame_with_format, FrameFormat};
        let mut wire = Vec::new();
        let video1 = vec![0, 0, 0, 1, 0x65, 0xAA];
        write_frame_with_format(&mut wire, 1280, 720, FrameFormat::H264, true, &video1).unwrap();
        let audio = AudioFrame {
            seq: 7,
            sample_rate: 48000,
            channels: 2,
            opus: vec![9; 100],
        };
        write_audio_frame(&mut wire, &audio).unwrap();
        let video2 = vec![0, 0, 0, 1, 0x41, 0xBB];
        write_frame_with_format(&mut wire, 1280, 720, FrameFormat::H264, false, &video2).unwrap();

        // Simulate buffered read: parse via magic dispatch
        let mut pos = 0;
        // first video
        {
            let mut cur = Cursor::new(&wire[pos..]);
            let mut buf = Vec::new();
            let info = crate::read_frame_sync(&mut cur, &mut buf).unwrap();
            assert_eq!(info.format, FrameFormat::H264);
            assert_eq!(buf, video1);
            pos += cur.position() as usize;
        }
        // audio — via try_read
        {
            let (af, consumed) = try_read_audio_frame(&wire[pos..]).unwrap().unwrap();
            assert_eq!(af, audio);
            pos += consumed;
        }
        // second video
        {
            let mut cur = Cursor::new(&wire[pos..]);
            let mut buf = Vec::new();
            let info = crate::read_frame_sync(&mut cur, &mut buf).unwrap();
            assert_eq!(buf, video2);
            pos += cur.position() as usize;
        }
        assert_eq!(pos, wire.len());

        // truncated CLA1 must not consume next CLF2
        let mut truncated = Vec::new();
        truncated.extend_from_slice(AUDIO_MAGIC);
        truncated.extend_from_slice(&7u32.to_le_bytes());
        truncated.extend_from_slice(&48000u32.to_le_bytes());
        truncated.push(2);
        truncated.extend_from_slice(&100u32.to_le_bytes());
        truncated.extend_from_slice(&[9; 50]); // only half payload
        let mut mixed = truncated.clone();
        let mut vwire = Vec::new();
        write_frame_with_format(&mut vwire, 640, 480, FrameFormat::H264, true, &video1).unwrap();
        mixed.extend_from_slice(&vwire);
        assert!(try_read_audio_frame(&mixed).is_err());
        // next CLF2 still parseable at correct offset? truncated failed, so caller
        // should treat as error, not skip — but a correct CLA1 of full length would parse.
        // Verify full CLA1 does not confuse CLF2 magic
        assert_eq!(&vwire[0..4], crate::FRAME_MAGIC);
    }

    #[test]
    fn audio_disabled_does_not_break_video() {
        use crate::{write_frame_with_format, FrameFormat};
        let mut wire = Vec::new();
        let video = vec![0u8; 4 * 4 * 4];
        write_frame_with_format(&mut wire, 4, 4, FrameFormat::Bgra, true, &video).unwrap();
        let mut buf = Vec::new();
        let info = crate::read_frame_sync(&mut Cursor::new(&wire), &mut buf).unwrap();
        assert_eq!(buf, video);
        assert_eq!(info.format, FrameFormat::Bgra);
    }
}
