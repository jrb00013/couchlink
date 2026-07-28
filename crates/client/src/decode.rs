//! H.264 decode via openh264, mirroring the host's openh264 encode path
//! (crates/host/src/encode.rs). Runs on its own OS thread (see webrtc_player.rs) —
//! never call `decode` from an async context.

use anyhow::Result;
use openh264::decoder::{Decoder, DecoderConfig};
use openh264::formats::YUVSource;
use openh264::OpenH264API;
use std::time::Instant;
use tracing::{info, warn};

pub struct DecodedFrame {
    pub y_plane: Vec<u8>,
    pub u_plane: Vec<u8>,
    pub v_plane: Vec<u8>,
    pub width: u32,
    pub height: u32,
}

pub struct H264Decoder {
    decoder: Decoder,
    frame_count: u64,
    latencies_us: Vec<u64>,
}

const LATENCY_LOG_EVERY: usize = 120; // ~once every 2s at 60fps

impl H264Decoder {
    pub fn new() -> Result<Self> {
        let config = DecoderConfig::new();
        let decoder = Decoder::with_api_config(OpenH264API::from_source(), config)?;
        Ok(Self {
            decoder,
            frame_count: 0,
            latencies_us: Vec::with_capacity(LATENCY_LOG_EVERY),
        })
    }

    /// `annex_b_nal` may contain one or more Annex-B NAL units (start-code prefixed).
    /// Returns `Some(frame)` once a full picture has been decoded, `None` if this
    /// call only advanced decoder state (e.g. parameter sets) without emitting a frame.
    pub fn decode(&mut self, annex_b_nal: &[u8]) -> Result<Option<DecodedFrame>> {
        let start = Instant::now();
        let result = self.decoder.decode(annex_b_nal);
        let elapsed_us = start.elapsed().as_micros() as u64;

        let decoded = match result {
            Ok(Some(d)) => d,
            Ok(None) => return Ok(None),
            Err(e) => {
                warn!("h264 decode error, dropping frame: {e}");
                return Ok(None);
            }
        };

        self.latencies_us.push(elapsed_us);
        self.frame_count += 1;

        let (width, height) = decoded.dimensions();
        let (y_stride, u_stride, v_stride) = decoded.strides();
        let y_plane = copy_plane(decoded.y(), width, height, y_stride);
        let u_plane = copy_plane(decoded.u(), width / 2, height / 2, u_stride);
        let v_plane = copy_plane(decoded.v(), width / 2, height / 2, v_stride);

        if self.latencies_us.len() >= LATENCY_LOG_EVERY {
            self.log_latency_stats();
        }

        Ok(Some(DecodedFrame {
            y_plane,
            u_plane,
            v_plane,
            width: width as u32,
            height: height as u32,
        }))
    }

    fn log_latency_stats(&mut self) {
        self.latencies_us.sort_unstable();
        let p50 = self.latencies_us[self.latencies_us.len() / 2];
        let p99 = self.latencies_us[(self.latencies_us.len() * 99 / 100).min(self.latencies_us.len() - 1)];
        info!(
            "decode latency over {} frames: p50={:.1}ms p99={:.1}ms",
            self.latencies_us.len(),
            p50 as f64 / 1000.0,
            p99 as f64 / 1000.0
        );
        self.latencies_us.clear();
    }
}

fn copy_plane(src: &[u8], width: usize, height: usize, stride: usize) -> Vec<u8> {
    let mut out = vec![0u8; width * height];
    for row in 0..height {
        let src_start = row * stride;
        let dst_start = row * width;
        out[dst_start..dst_start + width].copy_from_slice(&src[src_start..src_start + width]);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use openh264::encoder::{Encoder, EncoderConfig};
    use openh264::formats::{RgbSliceU8, YUVBuffer};

    fn encode_one_solid_frame(width: usize, height: usize) -> Vec<u8> {
        let config = EncoderConfig::new();
        let mut encoder = Encoder::with_api_config(OpenH264API::from_source(), config).unwrap();
        let rgb = vec![80u8; width * height * 3];
        let yuv = YUVBuffer::from_rgb_source(RgbSliceU8::new(&rgb, (width, height)));
        let bitstream = encoder.encode(&yuv).unwrap();
        let mut out = Vec::new();
        for l in 0..bitstream.num_layers() {
            let layer = bitstream.layer(l).unwrap();
            for n in 0..layer.nal_count() {
                out.extend_from_slice(layer.nal_unit(n).unwrap());
            }
        }
        out
    }

    #[test]
    fn decodes_a_real_encoded_frame() {
        let width = 64;
        let height = 64;
        let annex_b = encode_one_solid_frame(width, height);

        let mut decoder = H264Decoder::new().unwrap();
        let frame = decoder
            .decode(&annex_b)
            .unwrap()
            .expect("first real frame should decode to a picture");

        assert_eq!(frame.width, width as u32);
        assert_eq!(frame.height, height as u32);
        assert_eq!(frame.y_plane.len(), width * height);
        assert_eq!(frame.u_plane.len(), width * height / 4);
        assert_eq!(frame.v_plane.len(), width * height / 4);
    }
}
