//! Ultra-low-latency video over WebRTC DataChannel (`CLVD`).
//!
//! Bypasses the RTP media path (and Chrome's jitter buffer) by shipping Annex-B
//! H.264 access units on an unordered, unreliable DataChannel. The browser
//! decodes with WebCodecs and paints immediately.
//!
//! Large IDRs are split into fragments — SCTP's negotiated maxMessageSize is
//! often ~64 KiB, and a 720p keyframe routinely exceeds that.

use bytes::{Buf, BufMut, BytesMut};
use thiserror::Error;

/// DataChannel label for live video NALs.
pub const VIDEO_CHANNEL: &str = "video";

/// ASCII magic.
pub const VIDEO_MAGIC: &[u8; 4] = b"CLVD";
pub const VIDEO_VERSION: u8 = 2;

pub const FLAG_KEYFRAME: u8 = 1 << 0;

/// Header: magic(4) + ver(1) + flags(1) + width(u16) + height(u16) + seq(u32)
///       + frag_idx(u16) + frag_count(u16)
pub const VIDEO_HEADER_LEN: usize = 18;

/// Stay under common SCTP maxMessageSize (65536) with margin for DTLS/SCTP overhead.
pub const VIDEO_MAX_FRAGMENT_PAYLOAD: usize = 14_000;

#[derive(Debug, Error)]
pub enum VideoCodecError {
    #[error("buffer too short")]
    Short,
    #[error("bad magic")]
    BadMagic,
    #[error("unsupported version {0}")]
    BadVersion(u8),
    #[error("bad fragment layout")]
    BadFragment,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VideoAccessUnit {
    pub seq: u32,
    pub width: u16,
    pub height: u16,
    pub keyframe: bool,
    pub annex_b: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VideoFragment {
    pub seq: u32,
    pub width: u16,
    pub height: u16,
    pub keyframe: bool,
    pub frag_idx: u16,
    pub frag_count: u16,
    pub payload: Vec<u8>,
}

impl VideoFragment {
    pub fn encode(&self, out: &mut BytesMut) {
        out.reserve(VIDEO_HEADER_LEN + self.payload.len());
        out.put_slice(VIDEO_MAGIC);
        out.put_u8(VIDEO_VERSION);
        let mut flags = 0u8;
        if self.keyframe {
            flags |= FLAG_KEYFRAME;
        }
        out.put_u8(flags);
        out.put_u16_le(self.width);
        out.put_u16_le(self.height);
        out.put_u32_le(self.seq);
        out.put_u16_le(self.frag_idx);
        out.put_u16_le(self.frag_count);
        out.put_slice(&self.payload);
    }

    pub fn decode(mut buf: &[u8]) -> Result<Self, VideoCodecError> {
        if buf.len() < VIDEO_HEADER_LEN {
            return Err(VideoCodecError::Short);
        }
        let mut magic = [0u8; 4];
        buf.copy_to_slice(&mut magic);
        if &magic != VIDEO_MAGIC {
            return Err(VideoCodecError::BadMagic);
        }
        let ver = buf.get_u8();
        if ver != VIDEO_VERSION {
            return Err(VideoCodecError::BadVersion(ver));
        }
        let flags = buf.get_u8();
        let width = buf.get_u16_le();
        let height = buf.get_u16_le();
        let seq = buf.get_u32_le();
        let frag_idx = buf.get_u16_le();
        let frag_count = buf.get_u16_le();
        if frag_count == 0 || frag_idx >= frag_count {
            return Err(VideoCodecError::BadFragment);
        }
        Ok(Self {
            seq,
            width,
            height,
            keyframe: flags & FLAG_KEYFRAME != 0,
            frag_idx,
            frag_count,
            payload: buf.to_vec(),
        })
    }
}

impl VideoAccessUnit {
    /// Encode as one or more CLVD fragments (always ≥1).
    pub fn encode_fragments(&self) -> Vec<BytesMut> {
        let payload = &self.annex_b;
        let chunk = VIDEO_MAX_FRAGMENT_PAYLOAD.max(1);
        let frag_count = payload.len().div_ceil(chunk).max(1) as u16;
        let mut out = Vec::with_capacity(frag_count as usize);
        for (i, piece) in payload.chunks(chunk).enumerate() {
            let frag = VideoFragment {
                seq: self.seq,
                width: self.width,
                height: self.height,
                keyframe: self.keyframe,
                frag_idx: i as u16,
                frag_count,
                payload: piece.to_vec(),
            };
            let mut buf = BytesMut::with_capacity(VIDEO_HEADER_LEN + piece.len());
            frag.encode(&mut buf);
            out.push(buf);
        }
        if out.is_empty() {
            let frag = VideoFragment {
                seq: self.seq,
                width: self.width,
                height: self.height,
                keyframe: self.keyframe,
                frag_idx: 0,
                frag_count: 1,
                payload: Vec::new(),
            };
            let mut buf = BytesMut::with_capacity(VIDEO_HEADER_LEN);
            frag.encode(&mut buf);
            out.push(buf);
        }
        out
    }
}

/// Reassemble unordered fragments for a single access unit.
#[derive(Debug, Default)]
pub struct FragmentAssembler {
    seq: Option<u32>,
    width: u16,
    height: u16,
    keyframe: bool,
    frag_count: u16,
    parts: Vec<Option<Vec<u8>>>,
}

impl FragmentAssembler {
    pub fn push(&mut self, frag: VideoFragment) -> Option<VideoAccessUnit> {
        if self.seq != Some(frag.seq) {
            self.seq = Some(frag.seq);
            self.width = frag.width;
            self.height = frag.height;
            self.keyframe = frag.keyframe;
            self.frag_count = frag.frag_count;
            self.parts = vec![None; frag.frag_count as usize];
        }
        if frag.frag_count != self.frag_count || frag.frag_idx as usize >= self.parts.len() {
            return None;
        }
        self.parts[frag.frag_idx as usize] = Some(frag.payload);
        if self.parts.iter().any(|p| p.is_none()) {
            return None;
        }
        let mut annex_b = Vec::new();
        for part in self.parts.drain(..) {
            if let Some(p) = part {
                annex_b.extend_from_slice(&p);
            }
        }
        let au = VideoAccessUnit {
            seq: frag.seq,
            width: self.width,
            height: self.height,
            keyframe: self.keyframe,
            annex_b,
        };
        self.seq = None;
        Some(au)
    }
}

/// True if Annex-B contains an IDR slice (NAL type 5).
pub fn annex_b_is_keyframe(data: &[u8]) -> bool {
    let mut i = 0;
    while i + 4 < data.len() {
        let start = if data[i..].starts_with(&[0, 0, 0, 1]) {
            i + 4
        } else if data[i..].starts_with(&[0, 0, 1]) {
            i + 3
        } else {
            i += 1;
            continue;
        };
        if start >= data.len() {
            break;
        }
        let nal_type = data[start] & 0x1f;
        if nal_type == 5 {
            return true;
        }
        i = start + 1;
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_single_fragment() {
        let au = VideoAccessUnit {
            seq: 7,
            width: 1280,
            height: 720,
            keyframe: true,
            annex_b: vec![0, 0, 0, 1, 0x65, 1, 2, 3],
        };
        let frags = au.encode_fragments();
        assert_eq!(frags.len(), 1);
        let frag = VideoFragment::decode(&frags[0]).unwrap();
        assert_eq!(frag.frag_count, 1);
        let mut asm = FragmentAssembler::default();
        let back = asm.push(frag).unwrap();
        assert_eq!(back, au);
    }

    #[test]
    fn roundtrip_multi_fragment_out_of_order() {
        let annex_b: Vec<u8> = (0..30_000).map(|i| (i % 251) as u8).collect();
        let au = VideoAccessUnit {
            seq: 9,
            width: 1920,
            height: 1080,
            keyframe: true,
            annex_b: annex_b.clone(),
        };
        let frags = au.encode_fragments();
        assert!(frags.len() > 1);
        let mut decoded: Vec<_> = frags
            .iter()
            .map(|b| VideoFragment::decode(b).unwrap())
            .collect();
        decoded.reverse();
        let mut asm = FragmentAssembler::default();
        let mut out = None;
        for f in decoded {
            if let Some(au) = asm.push(f) {
                out = Some(au);
            }
        }
        let back = out.expect("reassembled");
        assert_eq!(back.annex_b, annex_b);
        assert!(back.keyframe);
    }

    #[test]
    fn detects_idr() {
        assert!(annex_b_is_keyframe(&[0, 0, 0, 1, 0x65, 0]));
        assert!(!annex_b_is_keyframe(&[0, 0, 0, 1, 0x41, 0]));
    }
}
