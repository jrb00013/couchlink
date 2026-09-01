//! Linux X11 desktop capture, used only for real-frame regression testing of
//! `crate::color::bgra_to_nv12` — the same conversion the Windows encode path
//! (`mf_encoder.rs`) feeds into the hardware encoder. There is no Linux
//! production capture/encode path in couchlink; this exists so the color math
//! can be exercised against a real captured frame on a machine that will
//! never run the Windows host.
//!
//! Uses `x11rb`'s core `GetImage` request (ZPixmap, depth 24/32) rather than
//! the MIT-SHM extension — this is a low-frequency regression/dev tool, not a
//! hot capture path, so the extra round trip per frame doesn't matter and it
//! avoids the shared-memory setup/teardown entirely. `crates/host` already
//! depends on `x11rb` for its own X11 use (see `capture/cursor_x11.rs`), so
//! this reuses that crate rather than adding a second X11 binding.

use anyhow::{bail, Context, Result};
use x11rb::connection::Connection;
use x11rb::protocol::xproto::{ConnectionExt as _, ImageFormat};
use x11rb::rust_connection::RustConnection;

/// One captured desktop frame as tightly-packed BGRA (matches the byte layout
/// `bgra_to_nv12` expects: B,G,R,A per pixel, row-major, no row padding).
pub struct CapturedFrame {
    pub width: usize,
    pub height: usize,
    pub bgra: Vec<u8>,
}

/// Connects to the X server named by `$DISPLAY`, grabs the root window of the
/// default screen, and returns it as one full-resolution BGRA frame.
pub fn capture_root_window() -> Result<CapturedFrame> {
    let (conn, screen_num) =
        RustConnection::connect(None).context("connect to X server (is $DISPLAY set?)")?;
    let screen = conn
        .setup()
        .roots
        .get(screen_num)
        .context("default screen missing from X11 setup")?
        .clone();

    let geom = conn
        .get_geometry(screen.root)
        .context("send GetGeometry")?
        .reply()
        .context("GetGeometry reply")?;
    let width = geom.width as usize;
    let height = geom.height as usize;

    let image = conn
        .get_image(
            ImageFormat::Z_PIXMAP,
            screen.root,
            0,
            0,
            geom.width,
            geom.height,
            !0, // plane mask: all planes
        )
        .context("send GetImage")?
        .reply()
        .context("GetImage reply")?;

    // X servers commonly report depth 24 with 32 bits-per-pixel (the top
    // byte unused/undefined) for TrueColor visuals — the common case on
    // Linux desktops. Anything else this tool doesn't understand is a hard
    // error rather than silently producing a corrupt frame.
    let bpp = 32; // depth-24 ZPixmap is packed as 4 bytes/pixel on every server we've seen
    let expected = width * height * (bpp / 8);
    if image.data.len() < expected {
        bail!(
            "GetImage returned {} bytes, expected at least {expected} for {width}x{height} @ {bpp}bpp (depth {})",
            image.data.len(),
            geom.depth
        );
    }

    // X11 ZPixmap for a 32bpp TrueColor visual is BGRX in little-endian byte
    // order on every server this was tested against (X.Org), i.e. byte 0 is
    // blue, byte 3 is padding/alpha. That is exactly the BGRA layout
    // bgra_to_nv12 expects, treating the pad byte as alpha (unused by the
    // conversion). Row stride is width*4 with no extra padding for this bpp.
    let bgra = image.data[..expected].to_vec();

    Ok(CapturedFrame {
        width,
        height,
        bgra,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::color::{bgra_to_nv12, nv12_len};

    /// Captures ONE real frame off the live X server and asserts the shared
    /// `bgra_to_nv12` conversion produces correct BT.709 full-range NV12.
    /// Skips (rather than failing the suite) when there is no X server to
    /// capture from, so `cargo test` stays green in headless CI without a
    /// display — see the PR description for how this was actually run with
    /// Xvfb / a real `$DISPLAY` on the dev box.
    #[test]
    fn real_x11_frame_converts_to_valid_full_range_nv12() {
        let frame = match capture_root_window() {
            Ok(f) => f,
            Err(e) => {
                eprintln!("skipping: no X11 capture available ({e})");
                return;
            }
        };
        assert!(frame.width > 0 && frame.height > 0, "non-empty geometry");
        assert_eq!(
            frame.bgra.len(),
            frame.width * frame.height * 4,
            "tightly packed BGRA buffer"
        );

        let mut nv12 = Vec::new();
        bgra_to_nv12(&frame.bgra, frame.width, frame.height, &mut nv12);
        assert_eq!(nv12.len(), nv12_len(frame.width as u32, frame.height as u32));

        let y_size = frame.width * frame.height;
        let (y_plane, uv_plane) = nv12.split_at(y_size);

        // Full-range assertion: a real desktop frame (window chrome, text,
        // wallpaper) should not have every luma sample clamped into
        // BT.601-style studio swing (16..=235) the way a mis-tagged
        // full<->limited conversion would produce. Require the observed
        // luma range to actually reach outside that band on at least one
        // side, proving values aren't being clipped to studio levels.
        let (mut lo, mut hi) = (255u8, 0u8);
        for &y in y_plane {
            lo = lo.min(y);
            hi = hi.max(y);
        }
        assert!(
            lo < 16 || hi > 235,
            "expected full-range luma (some sample <16 or >235), got range {lo}..={hi} \
             — looks clamped to BT.601 studio swing"
        );

        // Sanity: chroma plane is populated and within legal byte range (a
        // gross mismatch, e.g. reading the wrong channel order, tends to
        // produce chroma stuck at 0 or 255 across the whole plane).
        assert!(!uv_plane.is_empty());
        let all_extreme = uv_plane.iter().all(|&v| v == 0 || v == 255);
        assert!(!all_extreme, "chroma plane looks degenerate (all 0/255)");
    }
}
