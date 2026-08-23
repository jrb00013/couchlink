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
pub const VIDEO_VERSION: u8 = 3;
pub const VIDEO_VERSION_V2: u8 = 2;

pub const FLAG_KEYFRAME: u8 = 1 << 0;

/// v2 header: magic(4) + ver(1) + flags(1) + width(u16) + height(u16) + seq(u32)
///          + frag_idx(u16) + frag_count(u16)
pub const VIDEO_HEADER_LEN_V2: usize = 18;
/// v3 adds stamp_us (u64 LE) after frag_count.
pub const VIDEO_HEADER_LEN: usize = 26;

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
    /// Host monotonic µs at capture-read. 0 = unknown / v2 peer.
    pub stamp_us: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VideoFragment {
    pub seq: u32,
    pub width: u16,
    pub height: u16,
    pub keyframe: bool,
    pub frag_idx: u16,
    pub frag_count: u16,
    pub stamp_us: u64,
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
        out.put_u64_le(self.stamp_us);
        out.put_slice(&self.payload);
    }

    pub fn decode(mut buf: &[u8]) -> Result<Self, VideoCodecError> {
        if buf.len() < VIDEO_HEADER_LEN_V2 {
            return Err(VideoCodecError::Short);
        }
        let mut magic = [0u8; 4];
        buf.copy_to_slice(&mut magic);
        if &magic != VIDEO_MAGIC {
            return Err(VideoCodecError::BadMagic);
        }
        let ver = buf.get_u8();
        if ver != VIDEO_VERSION && ver != VIDEO_VERSION_V2 {
            return Err(VideoCodecError::BadVersion(ver));
        }
        let header_len = if ver == VIDEO_VERSION {
            VIDEO_HEADER_LEN
        } else {
            VIDEO_HEADER_LEN_V2
        };
        if buf.len() + 5 < header_len {
            return Err(VideoCodecError::Short);
        }
        let flags = buf.get_u8();
        let width = buf.get_u16_le();
        let height = buf.get_u16_le();
        let seq = buf.get_u32_le();
        let frag_idx = buf.get_u16_le();
        let frag_count = buf.get_u16_le();
        let stamp_us = if ver == VIDEO_VERSION {
            if buf.len() < 8 {
                return Err(VideoCodecError::Short);
            }
            buf.get_u64_le()
        } else {
            0
        };
        // `frag_idx == frag_count` is legal: it marks the FEC parity fragment,
        // one slot past the last data index. Only a data-fragment index must
        // stay below frag_count — enforced by the assemblers below, not here,
        // so this stays a pure wire-level parser rather than baking in a
        // policy about what a valid *access unit* looks like.
        if frag_count == 0 || frag_idx > frag_count {
            return Err(VideoCodecError::BadFragment);
        }
        Ok(Self {
            seq,
            width,
            height,
            keyframe: flags & FLAG_KEYFRAME != 0,
            frag_idx,
            frag_count,
            stamp_us,
            payload: buf.to_vec(),
        })
    }
}

impl VideoAccessUnit {
    /// Encode as one or more CLVD fragments (always ≥1).
    pub fn encode_fragments(&self) -> Vec<BytesMut> {
        self.encode_fragments_impl(false)
    }

    /// Same as [`encode_fragments`], plus one XOR-parity fragment appended.
    ///
    /// The video DataChannel is unordered and unreliable, so a single dropped
    /// fragment currently costs a full keyframe request — a multi-frame stall
    /// while the request round-trips and the next IDR is generated. XOR parity
    /// recovers exactly one lost fragment with **no round trip**: send N data
    /// fragments plus their XOR, and any single missing one is `XOR of the
    /// rest, XOR parity`. Two or more losses in the same access unit still
    /// need a keyframe — this does not replace that path, it just removes it
    /// as the response to the single-loss case, which on a lightly-loaded
    /// link (see the online-path utilisation measurement) is the common one.
    ///
    /// Off by default: enabling this without a measured non-trivial loss rate
    /// spends bandwidth to fix a problem that may not exist.
    pub fn encode_fragments_with_fec(&self) -> Vec<BytesMut> {
        self.encode_fragments_impl(true)
    }

    fn encode_fragments_impl(&self, fec: bool) -> Vec<BytesMut> {
        let payload = &self.annex_b;
        let chunk = VIDEO_MAX_FRAGMENT_PAYLOAD.max(1);
        let frag_count = payload.len().div_ceil(chunk).max(1) as u16;
        let mut out = Vec::with_capacity(frag_count as usize + 1);
        let mut xor = vec![0u8; VIDEO_MAX_FRAGMENT_PAYLOAD];
        let mut last_len: u16 = 0;
        let mut n_data = 0u16;
        for (i, piece) in payload.chunks(chunk).enumerate() {
            if fec {
                for (x, b) in xor.iter_mut().zip(piece) {
                    *x ^= *b;
                }
                last_len = piece.len() as u16;
                n_data += 1;
            }
            let frag = VideoFragment {
                seq: self.seq,
                width: self.width,
                height: self.height,
                keyframe: self.keyframe,
                frag_idx: i as u16,
                frag_count,
                stamp_us: self.stamp_us,
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
                stamp_us: self.stamp_us,
                payload: Vec::new(),
            };
            let mut buf = BytesMut::with_capacity(VIDEO_HEADER_LEN);
            frag.encode(&mut buf);
            out.push(buf);
            return out;
        }
        if fec && n_data > 1 {
            // A single fragment has nothing to XOR against — a "parity" of one
            // piece is just that piece again, so skip it rather than double
            // the send for zero recovery benefit.
            let mut parity_payload = Vec::with_capacity(2 + VIDEO_MAX_FRAGMENT_PAYLOAD);
            parity_payload.extend_from_slice(&last_len.to_le_bytes());
            parity_payload.extend_from_slice(&xor);
            let parity = VideoFragment {
                seq: self.seq,
                width: self.width,
                height: self.height,
                keyframe: self.keyframe,
                frag_idx: frag_count,
                frag_count,
                stamp_us: self.stamp_us,
                payload: parity_payload,
            };
            let mut buf = BytesMut::with_capacity(VIDEO_HEADER_LEN + parity.payload.len());
            parity.encode(&mut buf);
            out.push(buf);
        }
        out
    }
}

/// Reassemble unordered fragments for a single access unit.
///
/// Recovers one missing data fragment via XOR parity when the sender used
/// [`VideoAccessUnit::encode_fragments_with_fec`] — see there for why.
#[derive(Debug, Default)]
pub struct FragmentAssembler {
    seq: Option<u32>,
    width: u16,
    height: u16,
    keyframe: bool,
    frag_count: u16,
    stamp_us: u64,
    parts: Vec<Option<Vec<u8>>>,
    parity: Option<Vec<u8>>,
}

impl FragmentAssembler {
    pub fn push(&mut self, frag: VideoFragment) -> Option<VideoAccessUnit> {
        if self.seq != Some(frag.seq) {
            self.seq = Some(frag.seq);
            self.width = frag.width;
            self.height = frag.height;
            self.keyframe = frag.keyframe;
            self.frag_count = frag.frag_count;
            self.stamp_us = frag.stamp_us;
            self.parts = vec![None; frag.frag_count as usize];
            self.parity = None;
        }
        if frag.frag_count != self.frag_count {
            return None;
        }
        if frag.frag_idx == frag.frag_count {
            // The parity fragment — one slot past the last data index.
            self.parity = Some(frag.payload);
        } else if (frag.frag_idx as usize) < self.parts.len() {
            self.parts[frag.frag_idx as usize] = Some(frag.payload);
        } else {
            return None;
        }

        let missing: Vec<usize> = self
            .parts
            .iter()
            .enumerate()
            .filter_map(|(i, p)| p.is_none().then_some(i))
            .collect();
        match missing.as_slice() {
            [] => {}
            [m] => {
                let Some(recovered) = recover_fragment(&self.parts, *m, self.parity.as_deref())
                else {
                    return None;
                };
                self.parts[*m] = Some(recovered);
            }
            _ => return None,
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
            stamp_us: self.stamp_us,
        };
        self.seq = None;
        self.parity = None;
        Some(au)
    }
}

/// Reconstruct the one fragment at `missing` from the rest plus parity.
///
/// `parity` payload is `[last_frag_len: u16 LE][XOR bytes]`. XOR-ing every
/// present fragment (zero-padded to `VIDEO_MAX_FRAGMENT_PAYLOAD`) against the
/// parity XOR leaves exactly the missing fragment, padded the same way; only
/// the last fragment in an access unit can be short, so its length has to be
/// carried explicitly to know where to trim.
fn recover_fragment(
    parts: &[Option<Vec<u8>>],
    missing: usize,
    parity: Option<&[u8]>,
) -> Option<Vec<u8>> {
    let parity = parity?;
    if parity.len() < 2 + VIDEO_MAX_FRAGMENT_PAYLOAD {
        return None;
    }
    let last_len = u16::from_le_bytes([parity[0], parity[1]]) as usize;
    let mut acc = vec![0u8; VIDEO_MAX_FRAGMENT_PAYLOAD];
    for (x, b) in acc.iter_mut().zip(&parity[2..]) {
        *x ^= *b;
    }
    for (i, part) in parts.iter().enumerate() {
        if i == missing {
            continue;
        }
        let p = part.as_ref()?;
        for (x, b) in acc.iter_mut().zip(p) {
            *x ^= *b;
        }
    }
    let want_len = if missing + 1 == parts.len() {
        last_len
    } else {
        VIDEO_MAX_FRAGMENT_PAYLOAD
    };
    if want_len > acc.len() {
        return None;
    }
    acc.truncate(want_len);
    Some(acc)
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
            stamp_us: 0,
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
            stamp_us: 0,
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

    fn multi_frag_au(seq: u32) -> VideoAccessUnit {
        let annex_b: Vec<u8> = (0..30_000).map(|i| (i % 251) as u8).collect();
        VideoAccessUnit {
            seq,
            width: 1920,
            height: 1080,
            keyframe: true,
            annex_b,
            stamp_us: 0,
        }
    }

    #[test]
    fn fec_roundtrip_with_no_loss_matches_plain_encode() {
        let au = multi_frag_au(1);
        let frags = au.encode_fragments_with_fec();
        assert!(
            frags.len() > au.encode_fragments().len(),
            "parity fragment must be appended"
        );
        let mut asm = FragmentAssembler::default();
        let mut out = None;
        for f in &frags {
            if let Some(a) = asm.push(VideoFragment::decode(f).unwrap()) {
                out = Some(a);
            }
        }
        assert_eq!(out.unwrap(), au);
    }

    #[test]
    fn fec_recovers_any_single_dropped_data_fragment() {
        let au = multi_frag_au(2);
        let frags = au.encode_fragments_with_fec();
        let decoded: Vec<VideoFragment> = frags
            .iter()
            .map(|b| VideoFragment::decode(b).unwrap())
            .collect();
        let n_data = decoded.iter().filter(|f| f.frag_idx < f.frag_count).count();
        assert!(n_data > 1, "test needs a genuinely multi-fragment AU");

        // Every data index, dropped one at a time — including the last
        // fragment, whose length is the whole reason last_len exists.
        for drop_idx in 0..n_data {
            let mut asm = FragmentAssembler::default();
            let mut out = None;
            for f in &decoded {
                let is_data = (f.frag_idx as usize) < n_data;
                if is_data && f.frag_idx as usize == drop_idx {
                    continue; // simulate this one lost in transit
                }
                if let Some(a) = asm.push(f.clone()) {
                    out = Some(a);
                }
            }
            let recovered =
                out.unwrap_or_else(|| panic!("did not recover dropped frag {drop_idx}"));
            assert_eq!(
                recovered, au,
                "recovered AU mismatch dropping frag {drop_idx}"
            );
        }
    }

    #[test]
    fn fec_never_fabricates_output_when_two_fragments_are_lost() {
        // Two losses in one AU cannot be recovered from one parity fragment.
        // The system's fallback is a keyframe request — this only has to
        // prove the assembler stays silent rather than emitting corrupt video.
        let au = multi_frag_au(3);
        let frags = au.encode_fragments_with_fec();
        let decoded: Vec<VideoFragment> = frags
            .iter()
            .map(|b| VideoFragment::decode(b).unwrap())
            .collect();
        let mut asm = FragmentAssembler::default();
        let mut out = None;
        for f in &decoded {
            if f.frag_idx == 0 || f.frag_idx == 1 {
                continue; // drop two data fragments
            }
            if let Some(a) = asm.push(f.clone()) {
                out = Some(a);
            }
        }
        assert!(
            out.is_none(),
            "must not complete — would be silently corrupt video"
        );
    }

    #[test]
    fn fec_skips_parity_for_a_single_fragment_access_unit() {
        let au = VideoAccessUnit {
            seq: 4,
            width: 100,
            height: 100,
            keyframe: false,
            annex_b: vec![1, 2, 3],
            stamp_us: 0,
        };
        // A parity of one fragment against nothing recovers nothing —
        // sending it would cost bandwidth for zero benefit.
        assert_eq!(au.encode_fragments_with_fec().len(), 1);
    }

    #[test]
    fn decode_accepts_parity_index_but_rejects_further_out_of_range() {
        let frag = VideoFragment {
            seq: 1,
            width: 1,
            height: 1,
            keyframe: false,
            frag_idx: 3,
            frag_count: 3,
            stamp_us: 0,
            payload: vec![0; 2 + VIDEO_MAX_FRAGMENT_PAYLOAD],
        };
        let mut buf = BytesMut::new();
        frag.encode(&mut buf);
        assert!(VideoFragment::decode(&buf).is_ok());

        let bad = VideoFragment {
            frag_idx: 4,
            ..frag
        };
        let mut buf = BytesMut::new();
        bad.encode(&mut buf);
        assert!(matches!(
            VideoFragment::decode(&buf),
            Err(VideoCodecError::BadFragment)
        ));
    }

    #[test]
    fn detects_idr() {
        assert!(annex_b_is_keyframe(&[0, 0, 0, 1, 0x65, 0]));
        assert!(!annex_b_is_keyframe(&[0, 0, 0, 1, 0x41, 0]));
    }

    #[test]
    fn v2_fragment_still_decodes_without_a_stamp() {
        let mut buf = BytesMut::new();
        buf.put_slice(VIDEO_MAGIC);
        buf.put_u8(VIDEO_VERSION_V2);
        buf.put_u8(FLAG_KEYFRAME);
        buf.put_u16_le(1280);
        buf.put_u16_le(720);
        buf.put_u32_le(3);
        buf.put_u16_le(0);
        buf.put_u16_le(1);
        buf.put_slice(&[9, 8, 7]);
        let frag = VideoFragment::decode(&buf).unwrap();
        assert_eq!(frag.stamp_us, 0);
        assert_eq!(frag.payload, vec![9, 8, 7]);
        let mut asm = FragmentAssembler::default();
        let au = asm.push(frag).unwrap();
        assert_eq!(au.stamp_us, 0);
        assert_eq!(au.annex_b, vec![9, 8, 7]);
    }

    #[test]
    fn v3_round_trip_preserves_stamp_and_does_not_eat_payload() {
        let au = VideoAccessUnit {
            seq: 7,
            width: 1280,
            height: 720,
            keyframe: true,
            annex_b: vec![0, 0, 0, 1, 0x65],
            stamp_us: 1_234_567,
        };
        let frags = au.encode_fragments();
        assert_eq!(frags[0].len(), VIDEO_HEADER_LEN + au.annex_b.len());
        let back = {
            let mut asm = FragmentAssembler::default();
            asm.push(VideoFragment::decode(&frags[0]).unwrap()).unwrap()
        };
        assert_eq!(back.stamp_us, 1_234_567);
        assert_eq!(back.annex_b, au.annex_b);
    }

    #[test]
    fn v3_header_is_26_bytes_and_fec_parity_still_recovers_one_loss() {
        let au = VideoAccessUnit {
            seq: 2,
            width: 1920,
            height: 1080,
            keyframe: true,
            annex_b: (0..30_000).map(|i| (i % 251) as u8).collect(),
            stamp_us: 99,
        };
        let frags = au.encode_fragments_with_fec();
        let decoded: Vec<VideoFragment> = frags
            .iter()
            .map(|b| VideoFragment::decode(b).unwrap())
            .collect();
        assert!(decoded.iter().all(|f| f.stamp_us == 99));
        let n_data = decoded.iter().filter(|f| f.frag_idx < f.frag_count).count();
        let mut asm = FragmentAssembler::default();
        let mut out = None;
        for f in &decoded {
            if f.frag_idx == 0 {
                continue;
            }
            if let Some(a) = asm.push(f.clone()) {
                out = Some(a);
            }
        }
        let back = out.expect("FEC recover with stamp");
        assert_eq!(back.stamp_us, 99);
        assert_eq!(back.annex_b, au.annex_b);
        assert!(n_data > 1);
    }
}
