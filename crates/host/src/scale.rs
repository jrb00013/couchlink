//! Naive BGRA nearest-neighbor scale toward stream preset resolution.

pub fn scale_bgra(
    src: &[u8],
    sw: usize,
    sh: usize,
    dw: usize,
    dh: usize,
) -> Vec<u8> {
    let mut out = vec![0u8; dw * dh * 4];
    if sw == 0 || sh == 0 || dw == 0 || dh == 0 {
        return out;
    }
    for y in 0..dh {
        let sy = y * sh / dh;
        for x in 0..dw {
            let sx = x * sw / dw;
            let si = (sy * sw + sx) * 4;
            let di = (y * dw + x) * 4;
            out[di..di + 4].copy_from_slice(&src[si..si + 4]);
        }
    }
    out
}
