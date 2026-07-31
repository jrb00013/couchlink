//! Wire format for Windows → WSL (or any) capture frames.
//!
//! Frames cross this link either as raw BGRA pixels or as already-encoded H.264.
//! Encoding on the Windows side is enormously cheaper end to end — a 720p BGRA
//! frame is 3.3MB and costs the host ~10ms to convert and encode, while the same
//! frame as H.264 is tens of kilobytes and costs the host nothing but a relay — so
//! the format is negotiated per frame rather than assumed.

#[cfg(windows)]
pub mod keep_rendering;
#[cfg(windows)]
pub mod gpu_convert;
#[cfg(windows)]
pub mod mf_encoder;

use anyhow::{bail, Context, Result};
use std::io::{Read, Write};

/// Bumped from CLFR when the header gained a format byte. A mismatched binary now
/// fails loudly on the magic instead of silently misreading the header.
pub const FRAME_MAGIC: &[u8; 4] = b"CLF2";

const MAX_PAYLOAD: usize = 64 * 1024 * 1024;

/// Sent by the host back up the capture socket to ask for an IDR. The link is a
/// plain TCP stream, so the reverse direction is free — and without it a player
/// joining mid-session waits for the encoder's own keyframe interval.
pub const REQUEST_IDR: u8 = b'I';

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FrameFormat {
    /// Raw BGRA8, `width * height * 4` bytes. The fallback path.
    Bgra,
    /// H.264 Annex-B NAL units, already encoded at the stated dimensions.
    H264,
}

impl FrameFormat {
    const fn as_byte(self) -> u8 {
        match self {
            Self::Bgra => 0,
            Self::H264 => 1,
        }
    }

    const fn from_byte(b: u8) -> Option<Self> {
        match b {
            0 => Some(Self::Bgra),
            1 => Some(Self::H264),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct FrameInfo {
    pub width: u32,
    pub height: u32,
    pub format: FrameFormat,
    /// Only meaningful for H264: this access unit is an IDR and can be decoded
    /// without any earlier frame.
    pub keyframe: bool,
}

impl FrameInfo {
    pub fn bgra(width: u32, height: u32) -> Self {
        Self {
            width,
            height,
            format: FrameFormat::Bgra,
            keyframe: true,
        }
    }
}

pub fn read_frame_sync(r: &mut impl Read, buf: &mut Vec<u8>) -> Result<FrameInfo> {
    let mut magic = [0u8; 4];
    r.read_exact(&mut magic).context("frame magic")?;
    if &magic != FRAME_MAGIC {
        bail!(
            "bad frame magic {magic:?} — rebuild couchlink-win-capture, the wire format changed"
        );
    }
    read_frame_body_sync(r, buf)
}

/// Read everything after the 4-byte magic. Split out so callers that poll for the
/// start of a frame (and tolerate a timeout there) can still demand the rest of the
/// frame arrive in one piece — a timeout mid-frame desyncs the stream.
pub fn read_frame_body_sync(r: &mut impl Read, buf: &mut Vec<u8>) -> Result<FrameInfo> {
    let mut head = [0u8; 14];
    r.read_exact(&mut head).context("frame header")?;
    let format = FrameFormat::from_byte(head[0])
        .with_context(|| format!("unknown frame format {}", head[0]))?;
    let keyframe = head[1] & 1 != 0;
    let width = u32::from_le_bytes(head[2..6].try_into()?);
    let height = u32::from_le_bytes(head[6..10].try_into()?);
    let len = u32::from_le_bytes(head[10..14].try_into()?) as usize;

    if len == 0 || len > MAX_PAYLOAD {
        bail!("invalid frame payload size {len}");
    }
    if format == FrameFormat::Bgra {
        // Raw frames have exactly one correct size; anything else means the stream
        // is desynchronised and every later frame would be garbage.
        let expected = width as usize * height as usize * 4;
        if len != expected {
            bail!("bgra frame size {len} != expected {expected} for {width}x{height}");
        }
    }
    buf.resize(len, 0);
    r.read_exact(buf).context("frame payload")?;
    Ok(FrameInfo {
        width,
        height,
        format,
        keyframe,
    })
}

pub fn write_frame_sync(
    w: &mut impl Write,
    width: u32,
    height: u32,
    payload: &[u8],
) -> Result<()> {
    write_frame_with_format(w, width, height, FrameFormat::Bgra, true, payload)
}

pub fn write_frame_with_format(
    w: &mut impl Write,
    width: u32,
    height: u32,
    format: FrameFormat,
    keyframe: bool,
    payload: &[u8],
) -> Result<()> {
    w.write_all(FRAME_MAGIC)?;
    w.write_all(&[format.as_byte(), u8::from(keyframe)])?;
    w.write_all(&width.to_le_bytes())?;
    w.write_all(&height.to_le_bytes())?;
    w.write_all(&(payload.len() as u32).to_le_bytes())?;
    w.write_all(payload)?;
    w.flush()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn bgra_roundtrip() {
        let pixels = vec![7u8; 4 * 4 * 4];
        let mut wire = Vec::new();
        write_frame_sync(&mut wire, 4, 4, &pixels).unwrap();
        let mut buf = Vec::new();
        let info = read_frame_sync(&mut Cursor::new(&wire), &mut buf).unwrap();
        assert_eq!((info.width, info.height), (4, 4));
        assert_eq!(info.format, FrameFormat::Bgra);
        assert_eq!(buf, pixels);
    }

    #[test]
    fn h264_roundtrip_preserves_the_keyframe_flag() {
        let nal = vec![0, 0, 0, 1, 0x65, 0xAA];
        for keyframe in [true, false] {
            let mut wire = Vec::new();
            write_frame_with_format(&mut wire, 1280, 720, FrameFormat::H264, keyframe, &nal)
                .unwrap();
            let mut buf = Vec::new();
            let info = read_frame_sync(&mut Cursor::new(&wire), &mut buf).unwrap();
            assert_eq!(info.format, FrameFormat::H264);
            assert_eq!(info.keyframe, keyframe);
            assert_eq!((info.width, info.height), (1280, 720));
            assert_eq!(buf, nal);
        }
    }

    /// H264 payloads are any size; only BGRA has a size implied by its dimensions.
    #[test]
    fn h264_payload_size_is_not_checked_against_dimensions() {
        let nal = vec![9u8; 1234];
        let mut wire = Vec::new();
        write_frame_with_format(&mut wire, 1280, 720, FrameFormat::H264, true, &nal).unwrap();
        let mut buf = Vec::new();
        assert!(read_frame_sync(&mut Cursor::new(&wire), &mut buf).is_ok());
    }

    #[test]
    fn a_mismatched_bgra_size_is_rejected() {
        let mut wire = Vec::new();
        write_frame_with_format(&mut wire, 640, 480, FrameFormat::Bgra, true, &[1, 2, 3, 4])
            .unwrap();
        let mut buf = Vec::new();
        let err = read_frame_sync(&mut Cursor::new(&wire), &mut buf).unwrap_err();
        assert!(format!("{err}").contains("expected"), "got: {err}");
    }

    /// A stale couchlink-win-capture.exe must fail on the magic, not misparse the
    /// header and stream garbage.
    #[test]
    fn the_old_magic_is_rejected_with_a_rebuild_hint() {
        let mut wire = b"CLFR".to_vec();
        wire.extend_from_slice(&[0u8; 20]);
        let mut buf = Vec::new();
        let err = read_frame_sync(&mut Cursor::new(&wire), &mut buf).unwrap_err();
        assert!(format!("{err}").contains("rebuild"), "got: {err}");
    }

    #[test]
    fn an_unknown_format_byte_is_rejected() {
        let mut wire = FRAME_MAGIC.to_vec();
        wire.extend_from_slice(&[99, 0]);
        wire.extend_from_slice(&4u32.to_le_bytes());
        wire.extend_from_slice(&4u32.to_le_bytes());
        wire.extend_from_slice(&16u32.to_le_bytes());
        wire.extend_from_slice(&[0u8; 16]);
        let mut buf = Vec::new();
        assert!(read_frame_sync(&mut Cursor::new(&wire), &mut buf).is_err());
    }
}
