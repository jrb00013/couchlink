//! Naive BGRA nearest-neighbor scale toward stream preset resolution.
//! Aspect ratio is preserved and the remainder letterboxed — a captured window is
//! rarely the preset's shape (e.g. 1920x1032 into 1920x1080), and stretching it
//! makes everything look squashed on the player's screen.

pub fn scale_bgra(src: &[u8], sw: usize, sh: usize, dw: usize, dh: usize) -> Vec<u8> {
    let mut out = vec![0u8; dw * dh * 4];
    if sw == 0 || sh == 0 || dw == 0 || dh == 0 {
        return out;
    }

    // Largest rect with the source's aspect ratio that fits the destination:
    // sw/sh vs dw/dh, cross-multiplied to stay in integers.
    let (box_w, box_h) = if sw * dh <= dw * sh {
        // Source is relatively taller — height fills, width is padded.
        ((sw * dh / sh).min(dw), dh)
    } else {
        // Source is relatively wider — width fills, height is padded.
        (dw, (sh * dw / sw).min(dh))
    };
    let (box_w, box_h) = (box_w.max(1), box_h.max(1));
    let off_x = (dw - box_w) / 2;
    let off_y = (dh - box_h) / 2;

    for y in 0..box_h {
        let sy = y * sh / box_h;
        for x in 0..box_w {
            let sx = x * sw / box_w;
            let si = (sy * sw + sx) * 4;
            let di = ((y + off_y) * dw + (x + off_x)) * 4;
            // Never panic on a short source: a capture whose dimensions changed
            // mid-stream can hand us fewer bytes than sw*sh*4 implies.
            let Some(px) = src.get(si..si + 4) else {
                continue;
            };
            out[di..di + 4].copy_from_slice(px);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn solid(w: usize, h: usize) -> Vec<u8> {
        vec![255u8; w * h * 4]
    }

    #[test]
    fn letterboxes_instead_of_stretching() {
        // 1920x1032 into 1920x1080 keeps full width and pads top/bottom.
        let out = scale_bgra(&solid(1920, 1032), 1920, 1032, 1920, 1080);
        assert_eq!(out.len(), 1920 * 1080 * 4);
        let bar = (1080 - (1920 * 1032 / 1920)) / 2;
        assert!(bar > 0, "expected letterbox bars");
        // Top row is padding, middle row is image.
        assert_eq!(out[0], 0);
        assert_eq!(out[(540 * 1920) * 4], 255);
    }

    #[test]
    fn pillarboxes_a_tall_source() {
        let out = scale_bgra(&solid(600, 800), 600, 800, 1920, 1080);
        assert_eq!(out.len(), 1920 * 1080 * 4);
        // Left edge padded, centre filled.
        assert_eq!(out[0], 0);
        assert_eq!(out[(540 * 1920 + 960) * 4], 255);
    }

    #[test]
    fn short_source_does_not_panic() {
        let out = scale_bgra(&solid(1920, 500), 1920, 1032, 1920, 1080);
        assert_eq!(out.len(), 1920 * 1080 * 4);
    }
}
