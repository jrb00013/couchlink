//! X11 cursor overlay for local Linux capture.
//!
//! `scrap` (XShmGetImage) never includes the pointer. Windows DXGI does
//! (`CursorCaptureSettings::WithCursor`), which is why the mouse shows on a
//! Windows host but not on native Linux.
//!
//! Latency policy:
//! - Cache the cursor *shape* (XFixes GetCursorImage) and refresh it at most
//!   every [`SHAPE_REFRESH`].
//! - Update *position* every frame via QueryPointer (cheap local round-trip).
//! - At open, probe GetCursorImage; if the median exceeds [`OPEN_RTT_BUDGET`],
//!   disable the overlay entirely (prefer no cursor over added stream latency).
//! - Pure blend cost is guarded by a unit regression test.

use std::time::{Duration, Instant};

use tracing::{info, warn};
use x11rb::connection::Connection;
use x11rb::protocol::randr::ConnectionExt as _;
use x11rb::protocol::xfixes::ConnectionExt as _;
use x11rb::protocol::xproto::{ConnectionExt as _, Window};
use x11rb::rust_connection::RustConnection;

/// How often to re-fetch cursor pixels from XFixes.
const SHAPE_REFRESH: Duration = Duration::from_millis(100);

/// If a GetCursorImage round-trip is slower than this at open, bail out.
/// Local X11 is typically well under 500µs; 1.5ms already eats ~9% of a 60fps budget.
const OPEN_RTT_BUDGET: Duration = Duration::from_micros(1_500);

/// Blend of a 64×64 sprite into 1080p must stay under this (CPU-only regression).
#[cfg(test)]
const BLEND_BUDGET: Duration = Duration::from_micros(300);

struct CachedCursor {
    /// Premultiplied ARGB CARD32 pixels from XFixes.
    pixels: Vec<u32>,
    width: u16,
    height: u16,
    xhot: u16,
    yhot: u16,
}

pub struct CursorOverlay {
    conn: RustConnection,
    root: Window,
    /// Capture region origin in root coordinates (multi-monitor).
    origin_x: i32,
    origin_y: i32,
    cached: Option<CachedCursor>,
    last_shape_at: Instant,
}

impl CursorOverlay {
    /// Best-effort open. Returns `None` when disabled, XFixes is missing, or
    /// the X11 round-trip probe exceeds the latency budget.
    pub fn try_open(capture_w: usize, capture_h: usize) -> Option<Self> {
        match std::env::var("COUCHLINK_CURSOR").ok().as_deref() {
            Some("0") | Some("false") | Some("off") | Some("no") => {
                info!("cursor overlay disabled (COUCHLINK_CURSOR)");
                return None;
            }
            _ => {}
        }

        let (conn, screen_num) = match x11rb::connect(None) {
            Ok(c) => c,
            Err(e) => {
                warn!("cursor overlay: X11 connect failed ({e}) — no mouse in stream");
                return None;
            }
        };

        if let Err(e) = conn
            .xfixes_query_version(5, 0)
            .map_err(|e| e.to_string())
            .and_then(|c| c.reply().map_err(|e| e.to_string()))
        {
            warn!("cursor overlay: XFixes unavailable ({e}) — no mouse in stream");
            return None;
        }

        let root = conn.setup().roots.get(screen_num)?.root;
        let (origin_x, origin_y) = primary_origin(&conn, screen_num, capture_w, capture_h);

        let mut overlay = Self {
            conn,
            root,
            origin_x,
            origin_y,
            cached: None,
            last_shape_at: Instant::now() - SHAPE_REFRESH,
        };

        // Latency gate: if XFixes GetCursorImage is too slow, don't tax every frame.
        if let Some(rtt) = overlay.probe_get_cursor_rtt() {
            if rtt > OPEN_RTT_BUDGET {
                warn!(
                    "cursor overlay: GetCursorImage median {rtt:?} > budget {OPEN_RTT_BUDGET:?} — disabling (no mouse rather than extra latency)"
                );
                return None;
            }
            info!(
                "cursor overlay ready (XFixes, shape cached ≤{SHAPE_REFRESH:?}), probe RTT {rtt:?}, origin ({origin_x},{origin_y}) {capture_w}x{capture_h}"
            );
        } else {
            warn!("cursor overlay: GetCursorImage probe failed — disabling");
            return None;
        }

        // Prime the shape cache so the first streamed frame has a cursor.
        overlay.refresh_shape();
        Some(overlay)
    }

    /// Overlay the cursor onto a tight BGRA frame. Position every frame;
    /// shape refresh is rate-limited.
    pub fn blend(&mut self, bgra: &mut [u8], width: usize, height: usize) {
        if width == 0 || height == 0 || bgra.len() < width * height * 4 {
            return;
        }

        if self.cached.is_none() || self.last_shape_at.elapsed() >= SHAPE_REFRESH {
            self.refresh_shape();
        }

        let Some(cursor) = self.cached.as_ref() else {
            return;
        };
        if cursor.width == 0 || cursor.height == 0 {
            return;
        }

        let (ptr_x, ptr_y) = self.pointer_position().unwrap_or_else(|| {
            // Fallback: last shape reply had a position — not stored; skip frame.
            (-1, -1)
        });
        if ptr_x < 0 {
            return;
        }

        let origin_x = ptr_x - cursor.xhot as i32 - self.origin_x;
        let origin_y = ptr_y - cursor.yhot as i32 - self.origin_y;
        blend_premultiplied_argb(
            bgra,
            width,
            height,
            &cursor.pixels,
            cursor.width as usize,
            cursor.height as usize,
            origin_x,
            origin_y,
        );
    }

    fn pointer_position(&self) -> Option<(i32, i32)> {
        let reply = self.conn.query_pointer(self.root).ok()?.reply().ok()?;
        Some((reply.root_x as i32, reply.root_y as i32))
    }

    fn refresh_shape(&mut self) {
        let Ok(cookie) = self.conn.xfixes_get_cursor_image() else {
            return;
        };
        let Ok(cursor) = cookie.reply() else {
            return;
        };
        self.cached = Some(CachedCursor {
            pixels: cursor.cursor_image,
            width: cursor.width,
            height: cursor.height,
            xhot: cursor.xhot,
            yhot: cursor.yhot,
        });
        self.last_shape_at = Instant::now();
    }

    fn probe_get_cursor_rtt(&self) -> Option<Duration> {
        const N: usize = 7;
        let mut samples = Vec::with_capacity(N);
        for _ in 0..N {
            let t0 = Instant::now();
            let reply = self.conn.xfixes_get_cursor_image().ok()?.reply().ok()?;
            let dt = t0.elapsed();
            // Touch the reply so the compiler cannot elide the round-trip.
            let _ = reply.width;
            samples.push(dt);
        }
        samples.sort_unstable();
        Some(samples[N / 2])
    }
}

/// Premultiplied ARGB sprite → BGRA destination. Pure function for tests.
pub fn blend_premultiplied_argb(
    bgra: &mut [u8],
    frame_w: usize,
    frame_h: usize,
    cursor: &[u32],
    cw: usize,
    ch: usize,
    origin_x: i32,
    origin_y: i32,
) {
    if cw == 0 || ch == 0 || cursor.len() < cw * ch {
        return;
    }
    for row in 0..ch as i32 {
        let fy = origin_y + row;
        if fy < 0 || fy >= frame_h as i32 {
            continue;
        }
        for col in 0..cw as i32 {
            let fx = origin_x + col;
            if fx < 0 || fx >= frame_w as i32 {
                continue;
            }
            let px = cursor[(row as usize) * cw + col as usize];
            let a = (px >> 24) & 0xff;
            if a == 0 {
                continue;
            }
            let (sr, sg, sb) = ((px >> 16) & 0xff, (px >> 8) & 0xff, px & 0xff);
            let idx = (fy as usize * frame_w + fx as usize) * 4;
            bgra[idx] = (sb + bgra[idx] as u32 * (255 - a) / 255) as u8;
            bgra[idx + 1] = (sg + bgra[idx + 1] as u32 * (255 - a) / 255) as u8;
            bgra[idx + 2] = (sr + bgra[idx + 2] as u32 * (255 - a) / 255) as u8;
            bgra[idx + 3] = 255;
        }
    }
}

fn primary_origin(
    conn: &RustConnection,
    screen_num: usize,
    capture_w: usize,
    capture_h: usize,
) -> (i32, i32) {
    let setup = conn.setup();
    let Some(screen) = setup.roots.get(screen_num) else {
        return (0, 0);
    };
    if screen.width_in_pixels as usize == capture_w
        && screen.height_in_pixels as usize == capture_h
    {
        return (0, 0);
    }

    let Ok(cookie) = conn.randr_get_monitors(screen.root, true) else {
        return (0, 0);
    };
    let Ok(reply) = cookie.reply() else {
        return (0, 0);
    };
    let mut fallback = (0, 0);
    for mon in reply.monitors {
        if mon.width as usize == capture_w && mon.height as usize == capture_h {
            fallback = (mon.x as i32, mon.y as i32);
            if mon.primary {
                return fallback;
            }
        }
    }
    fallback
}

#[cfg(test)]
mod tests {
    use super::*;

    fn opaque_white_cursor(w: usize, h: usize) -> Vec<u32> {
        // Premultiplied opaque white ARGB.
        vec![0xffff_ffff; w * h]
    }

    #[test]
    fn blend_draws_visible_pixels() {
        let fw = 64;
        let fh = 64;
        let mut frame = vec![0u8; fw * fh * 4];
        let cursor = opaque_white_cursor(8, 8);
        blend_premultiplied_argb(&mut frame, fw, fh, &cursor, 8, 8, 10, 12);
        let idx = (12 * fw + 10) * 4;
        assert!(frame[idx] > 200 && frame[idx + 1] > 200 && frame[idx + 2] > 200);
    }

    /// Regression: cursor blend must stay negligible vs a 60fps frame budget.
    /// If this trips, prefer disabling the overlay over shipping added latency.
    #[test]
    fn blend_stays_within_latency_budget() {
        let fw = 1920;
        let fh = 1080;
        let mut frame = vec![40u8; fw * fh * 4];
        let cursor = opaque_white_cursor(64, 64);

        // Warm-up
        blend_premultiplied_argb(&mut frame, fw, fh, &cursor, 64, 64, 100, 100);

        const N: usize = 50;
        let mut total = Duration::ZERO;
        for i in 0..N {
            let t0 = Instant::now();
            blend_premultiplied_argb(
                &mut frame,
                fw,
                fh,
                &cursor,
                64,
                64,
                100 + (i as i32 % 50),
                100,
            );
            total += t0.elapsed();
        }
        let avg = total / N as u32;
        assert!(
            avg <= BLEND_BUDGET,
            "cursor blend avg {avg:?} exceeds budget {BLEND_BUDGET:?} — disable overlay rather than add latency"
        );
    }
}
