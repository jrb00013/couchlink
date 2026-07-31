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
    /// When decode finished, so the viewer can report how long a finished frame
    /// waited before it was actually on screen.
    pub decoded_at: Instant,
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
    decode_errors: u64,
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
            decode_errors: 0,
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
                // One line per lost frame turns normal connection warm-up into what
                // looks like a catastrophic failure: while ICE and DTLS settle, some
                // packets are lost, the NALs built from them are incomplete, and the
                // decoder rejects them until the next keyframe. Measured on a healthy
                // link this is a few hundred frames over the first seconds and then
                // nothing at all, so report it as a rate-limited summary.
                self.decode_errors += 1;
                if self.decode_errors == 1 || self.decode_errors % 250 == 0 {
                    warn!(
                        "h264 decode errors: {} so far (expected while the connection \
                         settles; persistent counts mean real loss) — last: {e}",
                        self.decode_errors
                    );
                }
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
            decoded_at: Instant::now(),
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

    fn encode_solid_bgra(width: usize, height: usize, b: u8, g: u8, r: u8) -> Vec<u8> {
        // Host-style limited-range BT.601 → OpenH264, same path as production.
        use openh264::encoder::{Encoder, EncoderConfig};
        use openh264::formats::YUVSource;

        struct I420<'a> {
            data: &'a [u8],
            w: usize,
            h: usize,
        }
        impl YUVSource for I420<'_> {
            fn dimensions(&self) -> (usize, usize) {
                (self.w, self.h)
            }
            fn strides(&self) -> (usize, usize, usize) {
                (self.w, self.w.div_ceil(2), self.w.div_ceil(2))
            }
            fn y(&self) -> &[u8] {
                &self.data[..self.w * self.h]
            }
            fn u(&self) -> &[u8] {
                let y = self.w * self.h;
                let c = self.w.div_ceil(2) * self.h.div_ceil(2);
                &self.data[y..y + c]
            }
            fn v(&self) -> &[u8] {
                let y = self.w * self.h;
                let c = self.w.div_ceil(2) * self.h.div_ceil(2);
                &self.data[y + c..]
            }
        }

        fn bgra_to_i420(bgra: &[u8], width: usize, height: usize) -> Vec<u8> {
            let cw = width.div_ceil(2);
            let ch = height.div_ceil(2);
            let mut out = vec![0u8; width * height + 2 * cw * ch];
            let (y_plane, uv) = out.split_at_mut(width * height);
            let (u_plane, v_plane) = uv.split_at_mut(cw * ch);
            for y in 0..height {
                for x in 0..width {
                    let i = (y * width + x) * 4;
                    let bb = bgra[i] as i32;
                    let gg = bgra[i + 1] as i32;
                    let rr = bgra[i + 2] as i32;
                    y_plane[y * width + x] =
                        ((((66 * rr + 129 * gg + 25 * bb + 128) >> 8) + 16).clamp(0, 255)) as u8;
                    if y % 2 == 0 && x % 2 == 0 {
                        let cx = x / 2;
                        let cy = y / 2;
                        u_plane[cy * cw + cx] =
                            ((((-38 * rr - 74 * gg + 112 * bb + 128) >> 8) + 128).clamp(0, 255)) as u8;
                        v_plane[cy * cw + cx] =
                            ((((112 * rr - 94 * gg - 18 * bb + 128) >> 8) + 128).clamp(0, 255)) as u8;
                    }
                }
            }
            out
        }

        let mut bgra = vec![0u8; width * height * 4];
        for px in bgra.chunks_exact_mut(4) {
            px[0] = b;
            px[1] = g;
            px[2] = r;
            px[3] = 255;
        }
        let i420 = bgra_to_i420(&bgra, width, height);
        let config = EncoderConfig::new();
        let mut encoder = Encoder::with_api_config(OpenH264API::from_source(), config).unwrap();
        let src = I420 {
            data: &i420,
            w: width,
            h: height,
        };
        let bitstream = encoder.encode(&src).unwrap();
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
    fn saturated_red_keeps_chroma_not_grayscale() {
        let annex_b = encode_solid_bgra(64, 64, 0, 0, 255);
        let mut decoder = H264Decoder::new().unwrap();
        let frame = decoder
            .decode(&annex_b)
            .unwrap()
            .expect("red frame should decode");

        let avg_u =
            frame.u_plane.iter().map(|&x| x as u32).sum::<u32>() / frame.u_plane.len() as u32;
        let avg_v =
            frame.v_plane.iter().map(|&x| x as u32).sum::<u32>() / frame.v_plane.len() as u32;
        // Neutral gray sits at ~128. Saturated red is high V, low-ish U.
        assert!(
            avg_v > 140,
            "expected red chroma (high V), got avg U={avg_u} V={avg_v} — would present as grayish"
        );
        assert!(
            (avg_u as i32 - 128).abs() > 10 || avg_v > 160,
            "chroma collapsed toward gray (U={avg_u} V={avg_v})"
        );
    }
}
