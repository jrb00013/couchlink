//! OpenH264 encode path targeting HD low-latency (baseline, low latency tune).

use anyhow::{Context, Result};
use openh264::encoder::{Encoder, EncoderConfig};
use openh264::formats::YUVBuffer;
use openh264::OpenH264API;

pub struct H264Encoder {
    enc: Encoder,
    pub width: u32,
    pub height: u32,
    pub bitrate_kbps: u32,
}

impl H264Encoder {
    pub fn new(width: u32, height: u32, bitrate_kbps: u32) -> Result<Self> {
        let api = OpenH264API::from_source();
        let cfg = EncoderConfig::new(width, height)
            .max_frame_rate(60.0)
            .bitrate_bps((bitrate_kbps * 1000).max(1_000_000));
        let enc = Encoder::with_api_config(api, cfg).context("openh264 encoder")?;
        Ok(Self {
            enc,
            width,
            height,
            bitrate_kbps,
        })
    }

    /// Encode BGRA frame → Annex-B access unit bytes.
    pub fn encode_bgra(&mut self, bgra: &[u8]) -> Result<Option<Vec<u8>>> {
        let w = self.width as usize;
        let h = self.height as usize;
        if bgra.len() < w * h * 4 {
            return Ok(None);
        }
        // Convert BGRA → RGB for YUVBuffer helper
        let mut rgb = vec![0u8; w * h * 3];
        for i in 0..(w * h) {
            rgb[i * 3] = bgra[i * 4 + 2];
            rgb[i * 3 + 1] = bgra[i * 4 + 1];
            rgb[i * 3 + 2] = bgra[i * 4];
        }
        let yuv = YUVBuffer::from_rgb8(w, h, &rgb);
        let bitstream = self.enc.encode(&yuv).context("encode")?;
        let mut out = Vec::new();
        bitstream.write_vec(&mut out);
        if out.is_empty() {
            Ok(None)
        } else {
            Ok(Some(out))
        }
    }
}
