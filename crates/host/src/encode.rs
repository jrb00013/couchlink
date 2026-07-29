//! OpenH264 encode path targeting HD low-latency (screen realtime tune).
//! Method mirrors Rohomieo host encode (BGRA → I420 → Annex-B).

use anyhow::Result;
use openh264::encoder::{EncodedBitStream, Encoder, EncoderConfig, RateControlMode, SpsPpsStrategy, UsageType};
use openh264::formats::YUVSource;
use openh264::Error;
use openh264::OpenH264API;

const ANNEX_B_START: &[u8] = &[0, 0, 0, 1];

pub fn annex_b_from_bitstream(bs: &EncodedBitStream<'_>) -> Vec<u8> {
    let mut out = Vec::new();
    for l in 0..bs.num_layers() {
        let Some(layer) = bs.layer(l) else {
            continue;
        };
        for n in 0..layer.nal_count() {
            let Some(nal) = layer.nal_unit(n) else {
                continue;
            };
            if !nal.starts_with(ANNEX_B_START) && !nal.starts_with(&[0, 0, 1]) {
                out.extend_from_slice(ANNEX_B_START);
            }
            out.extend_from_slice(nal);
        }
    }
    out
}

/// BGRA -> I420 (BT.601). This runs on every frame, over ~1M pixels, so it is the
/// single largest CPU cost in the host. Bounds are resolved once per row rather than
/// per pixel, and chroma is written on even rows only instead of testing `x % 2` a
/// million times a frame.
pub fn bgra_to_i420(bgra: &[u8], width: usize, height: usize, stride: usize) -> Vec<u8> {
    let chroma_w = width.div_ceil(2);
    let chroma_h = height.div_ceil(2);
    let mut i420 = vec![0u8; width * height + 2 * chroma_w * chroma_h];
    if width == 0 || height == 0 {
        return i420;
    }
    let (y_plane, uv) = i420.split_at_mut(width * height);
    let (u_plane, v_plane) = uv.split_at_mut(chroma_w * chroma_h);

    for y in 0..height {
        // One bounds check per row. A short source (capture resized mid-stream)
        // leaves the remaining rows as the zeros they were initialised to.
        let Some(row) = bgra.get(y * stride..y * stride + width * 4) else {
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
        let cy = y / 2;
        let u_row = &mut u_plane[cy * chroma_w..(cy + 1) * chroma_w];
        let v_row = &mut v_plane[cy * chroma_w..(cy + 1) * chroma_w];
        for (cx, (u_out, v_out)) in u_row.iter_mut().zip(v_row.iter_mut()).enumerate() {
            // Odd widths: the final chroma sample has no second column to pair with.
            let i = cx * 8;
            let px = &row[i..i + 4];
            let b = px[0] as i32;
            let g = px[1] as i32;
            let r = px[2] as i32;
            *u_out = ((((-38 * r - 74 * g + 112 * b + 128) >> 8) + 128).clamp(0, 255)) as u8;
            *v_out = ((((112 * r - 94 * g - 18 * b + 128) >> 8) + 128).clamp(0, 255)) as u8;
        }
    }
    i420
}

struct I420Buffer<'a> {
    data: &'a [u8],
    width: usize,
    height: usize,
}

impl YUVSource for I420Buffer<'_> {
    fn dimensions(&self) -> (usize, usize) {
        (self.width, self.height)
    }

    fn strides(&self) -> (usize, usize, usize) {
        let chroma_w = (self.width + 1) / 2;
        (self.width, chroma_w, chroma_w)
    }

    fn y(&self) -> &[u8] {
        &self.data[..self.width * self.height]
    }

    fn u(&self) -> &[u8] {
        let y_len = self.width * self.height;
        let chroma_w = (self.width + 1) / 2;
        let chroma_h = (self.height + 1) / 2;
        &self.data[y_len..y_len + chroma_w * chroma_h]
    }

    fn v(&self) -> &[u8] {
        let y_len = self.width * self.height;
        let chroma_w = (self.width + 1) / 2;
        let chroma_h = (self.height + 1) / 2;
        &self.data[y_len + chroma_w * chroma_h..]
    }
}

pub struct H264Encoder {
    enc: Encoder,
    pub width: u32,
    pub height: u32,
    pub bitrate_kbps: u32,
}

impl H264Encoder {
    pub fn new(width: u32, height: u32, bitrate_kbps: u32) -> Result<Self> {
        let bps = (bitrate_kbps.saturating_mul(1000)).max(1_000_000);
        let config = EncoderConfig::new()
            .set_bitrate_bps(bps)
            .max_frame_rate(60.0)
            .usage_type(UsageType::ScreenContentRealTime)
            .rate_control_mode(RateControlMode::Bitrate)
            .enable_skip_frame(false)
            .sps_pps_strategy(SpsPpsStrategy::IncreasingId);
        let enc = Encoder::with_api_config(OpenH264API::from_source(), config)
            .map_err(|e: Error| anyhow::anyhow!("{e}"))?;
        Ok(Self {
            enc,
            width,
            height,
            bitrate_kbps,
        })
    }

    /// Ask OpenH264 for an IDR on the next encode (browser needs SPS/PPS+IDR to start).
    pub fn force_keyframe(&mut self) {
        self.enc.force_intra_frame();
    }

    pub fn encode_bgra(&mut self, bgra: &[u8]) -> Result<Option<Vec<u8>>> {
        let w = self.width as usize;
        let h = self.height as usize;
        let stride = w * 4;
        if bgra.len() < stride * h {
            return Ok(None);
        }
        let i420 = bgra_to_i420(bgra, w, h, stride);
        let src = I420Buffer {
            data: &i420,
            width: w,
            height: h,
        };
        let bitstream = self
            .enc
            .encode(&src)
            .map_err(|e: Error| anyhow::anyhow!("{e}"))?;
        let out = annex_b_from_bitstream(&bitstream);
        if out.is_empty() {
            Ok(None)
        } else {
            Ok(Some(out))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The original per-pixel implementation, kept only as the oracle for the
    /// optimised one. Any divergence is a colour bug, which is exactly the kind of
    /// thing a "harmless" optimisation ships by accident.
    fn reference_bgra_to_i420(
        bgra: &[u8],
        width: usize,
        height: usize,
        stride: usize,
    ) -> Vec<u8> {
        let chroma_w = (width + 1) / 2;
        let chroma_h = (height + 1) / 2;
        let mut i420 = vec![0u8; width * height + 2 * chroma_w * chroma_h];
        let (y_plane, uv) = i420.split_at_mut(width * height);
        let (u_plane, v_plane) = uv.split_at_mut(chroma_w * chroma_h);
        for y in 0..height {
            for x in 0..width {
                let i = y * stride + x * 4;
                if i + 3 >= bgra.len() {
                    continue;
                }
                let b = bgra[i] as i32;
                let g = bgra[i + 1] as i32;
                let r = bgra[i + 2] as i32;
                let y_val = ((66 * r + 129 * g + 25 * b + 128) >> 8) + 16;
                y_plane[y * width + x] = y_val.clamp(0, 255) as u8;
                if x % 2 == 0 && y % 2 == 0 {
                    let u_val = ((-38 * r - 74 * g + 112 * b + 128) >> 8) + 128;
                    let v_val = ((112 * r - 94 * g - 18 * b + 128) >> 8) + 128;
                    let uv_idx = (y / 2) * chroma_w + (x / 2);
                    if uv_idx < u_plane.len() {
                        u_plane[uv_idx] = u_val.clamp(0, 255) as u8;
                        v_plane[uv_idx] = v_val.clamp(0, 255) as u8;
                    }
                }
            }
        }
        i420
    }

    /// Deterministic pseudo-random pixels — a flat colour would hide index bugs.
    fn noise(w: usize, h: usize) -> Vec<u8> {
        let mut seed = 0x9E3779B9u32;
        (0..w * h * 4)
            .map(|_| {
                seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
                (seed >> 24) as u8
            })
            .collect()
    }

    #[test]
    fn matches_the_reference_implementation() {
        // Even, odd, and non-square dimensions: odd widths are where the chroma
        // pairing has no second column to average with.
        for (w, h) in [(16, 16), (15, 9), (1, 1), (2, 3), (64, 33), (128, 72)] {
            let src = noise(w, h);
            let stride = w * 4;
            assert_eq!(
                bgra_to_i420(&src, w, h, stride),
                reference_bgra_to_i420(&src, w, h, stride),
                "diverged at {w}x{h}"
            );
        }
    }

    #[test]
    fn short_source_does_not_panic() {
        let w = 64;
        let h = 64;
        let truncated = noise(w, h / 2);
        let out = bgra_to_i420(&truncated, w, h, w * 4);
        assert_eq!(out.len(), w * h + 2 * (w / 2) * (h / 2));
    }

    #[test]
    fn known_colours_convert_correctly() {
        // Pure white and pure black at the BT.601 studio-swing limits.
        let white = vec![255u8; 4 * 4 * 4];
        let out = bgra_to_i420(&white, 4, 4, 16);
        assert_eq!(out[0], 235, "white luma should be 235 (studio swing)");
        let black = vec![0u8; 4 * 4 * 4];
        let out = bgra_to_i420(&black, 4, 4, 16);
        assert_eq!(out[0], 16, "black luma should be 16 (studio swing)");
    }
}
