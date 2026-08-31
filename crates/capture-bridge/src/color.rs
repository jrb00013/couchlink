//! BGRA -> NV12 color conversion, shared by the Windows Media Foundation
//! encode path (`mf_encoder.rs`) and the Linux X11 capture regression path
//! (`linux_capture.rs`). Platform-agnostic on purpose: keep exactly one
//! implementation of the color math so the two capture backends can never
//! drift apart on primaries/range.

pub const fn nv12_len(width: u32, height: u32) -> usize {
    (width as usize * height as usize) + (width as usize * height as usize / 2)
}

/// BGRA -> NV12 (BT.709, full range), the input format hardware encoders
/// universally accept. Capture is PC-level RGB off a BT.709 desktop; encoding
/// it with BT.601 studio-swing coefficients is what reads as washed / tinted
/// chroma. Coefficients and range must match `MF_MT_YUV_MATRIX` /
/// `MF_MT_VIDEO_NOMINAL_RANGE` set on the Windows encoder input type, and the
/// decode-side colorSpace tagging in `webCodecsCanvas.ts` / the native client
/// shader — all four have to agree or the fix just moves where the tint is.
/// Chroma is interleaved (UVUV) and half resolution in both axes.
pub fn bgra_to_nv12(bgra: &[u8], width: usize, height: usize, out: &mut Vec<u8>) {
    let y_size = width * height;
    out.resize(y_size + y_size / 2, 0);
    let (y_plane, uv_plane) = out.split_at_mut(y_size);
    let stride = width * 4;

    for y in 0..height {
        let Some(row) = bgra.get(y * stride..y * stride + stride) else {
            break;
        };
        let y_row = &mut y_plane[y * width..(y + 1) * width];
        for (px, out) in row.chunks_exact(4).zip(y_row.iter_mut()) {
            let b = px[0] as i32;
            let g = px[1] as i32;
            let r = px[2] as i32;
            // Coefficients must sum to 256 (the >> 8 scale) or pure white
            // undershoots 255 by the shortfall. 54+183+18=255 was a real bug
            // here (caught by the Linux X11 regression test, since this code
            // only ever compiled on Windows before) — bumped blue (least
            // visually sensitive channel) to 19 to close the gap.
            *out = ((54 * r + 183 * g + 19 * b + 128) >> 8).clamp(0, 255) as u8;
        }
        if y % 2 != 0 {
            continue;
        }
        let uv_row = &mut uv_plane[(y / 2) * width..(y / 2) * width + width];
        for (cx, uv) in uv_row.chunks_exact_mut(2).enumerate() {
            let px = &row[cx * 8..cx * 8 + 4];
            let b = px[0] as i32;
            let g = px[1] as i32;
            let r = px[2] as i32;
            uv[0] =
                ((((-29 * r - 99 * g + 128 * b + 128) >> 8) + 128).clamp(0, 255)) as u8;
            uv[1] =
                ((((128 * r - 116 * g - 12 * b + 128) >> 8) + 128).clamp(0, 255)) as u8;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nv12_is_the_right_size_and_luma_matches_bt709_full_range() {
        let white = vec![255u8; 4 * 4 * 4];
        let mut out = Vec::new();
        bgra_to_nv12(&white, 4, 4, &mut out);
        assert_eq!(out.len(), nv12_len(4, 4));
        assert_eq!(out[0], 255, "white luma is 255 in full range");
        let black = vec![0u8; 4 * 4 * 4];
        bgra_to_nv12(&black, 4, 4, &mut out);
        assert_eq!(out[0], 0, "black luma is 0 in full range");
        // Neutral chroma for greyscale input.
        assert_eq!(out[16], 128);
        assert_eq!(out[17], 128);
    }

    #[test]
    fn a_short_source_does_not_panic() {
        let short = vec![0u8; 4 * 2 * 4];
        let mut out = Vec::new();
        bgra_to_nv12(&short, 4, 4, &mut out);
        assert_eq!(out.len(), nv12_len(4, 4));
    }
}
