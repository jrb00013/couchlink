//! OpenH264 encode path targeting HD low-latency (screen realtime tune).
//! Method mirrors Rohomieo host encode (BGRA → I420 → Annex-B).

use anyhow::Result;
use openh264::encoder::{EncodedBitStream, Encoder, EncoderConfig, RateControlMode, UsageType};
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

pub fn bgra_to_i420(bgra: &[u8], width: usize, height: usize, stride: usize) -> Vec<u8> {
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
            .rate_control_mode(RateControlMode::Bitrate);
        let enc = Encoder::with_api_config(OpenH264API::from_source(), config)
            .map_err(|e: Error| anyhow::anyhow!("{e}"))?;
        Ok(Self {
            enc,
            width,
            height,
            bitrate_kbps,
        })
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
