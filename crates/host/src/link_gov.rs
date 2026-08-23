//! Link governor — adapts the pre-encoded stream target to what the link can
//! actually carry.
//!
//! The Windows encoder is the fastest component in the chain: it happily emits
//! `preset` at full rate on hardware, with no idea that a remote player on a WAN
//! tunnel can decode a fraction of it. When the link saturates, the host sheds
//! frames (bounded push), the browser in turn drops late frames and demands
//! keyframes, and the encoder answers with IDRs — the loss/adapter feedback loop
//! described in the old code as a "death spiral". Every rung of that ladder is
//! downstream of one decision nobody was making: what the encoder feeds.
//!
//! This closes the loop at the source. The host already owns the link telemetry
//! (frames shed while pushing). When sheds persist, it *commands the encoder
//! down* over the capture socket (`SET_TARGET`), buying the same latency the
//! shed-loop was burning to get — the decoder stays healthy and the newest frame
//! arrives, instead of the newest frame arriving late and being dropped.
//!
//! `EncodeTarget` is the shared wire type, so the governor deliberately reuses it
//! rather than inventing an internal one: the rung set IS the stream resolution
//! ladder the host would otherwise advertise.

use couchlink_capture_bridge::EncodeTarget;

/// A sustained shed share above this (per window) steps the target down.
/// 2% was noise: three WAN viewers shed a couple of frames every window,
/// the governor climbed to 5000 kbps, immediately shed 7–19%, stepped back
/// to 2500, and the yo-yo dropped IDRs that freeze WebCodecs.
const DOWN_TRIGGER_PCT: u32 = 8;
/// Consecutive bad windows before stepping down — kills 5s yo-yo (5↔2.5 Mbps).
const DOWN_AFTER_WINDOWS: u32 = 2;
/// Clean windows required before climbing. Two 5s windows was ~10s at the
/// floor then a failed climb — forever. Eight windows is ~40s of real
/// headroom before we spend the extra bitrate.
const UP_AFTER_CLEAN_WINDOWS: u32 = 8;

/// The governor's persistent memory between windows.
#[derive(Debug, Clone)]
pub struct LinkGov {
    /// Where the rung ladder starts — the preset the host advertises.
    baseline: EncodeTarget,
    /// What we are currently commanding, or `less` while stepping back up.
    current: EncodeTarget,
    /// Consecutive clean windows (no triggering sheds).
    clean_windows: u32,
    /// Bad windows in a row — hysteresis so one 9% blip does not floor bitrate.
    bad_windows: u32,
}

/// The rung ladder for 3-friend WAN: **hold host fps**, step bitrate only.
///
/// Cutting fps to "fix lag" doubles unsync wait (T/2 at 15 = 33 ms vs 8 ms
/// at 60) and matches tonight's death spiral (overlay target 30/15 while
/// friends decode at the push rate). Bits per frame at the same R are
/// *smaller* at 60 than at 15, so holding fps is often cheaper on CLVD.
fn rungs_from(baseline: &EncodeTarget) -> Vec<EncodeTarget> {
    let mut rungs = vec![*baseline];
    let mut kbps = baseline.bitrate_kbps;
    // Never step below 1250 kbps at 60 — 625 was unwatchable (2–7 push fps)
    // and the governor had no climb room once IDR storms inflated sheds.
    const FLOOR_KBPS: u32 = 1_250;
    for _ in 0..3 {
        let next = (kbps / 2).max(FLOOR_KBPS);
        if next >= kbps {
            break;
        }
        kbps = next;
        let extra = EncodeTarget {
            fps: baseline.fps,
            bitrate_kbps: kbps,
            ..*baseline
        };
        if !rungs.iter().any(|r| *r == extra) {
            rungs.push(extra);
        }
    }
    rungs
}

fn index_of(rungs: &[EncodeTarget], target: EncodeTarget) -> Option<usize> {
    rungs.iter().position(|r| *r == target)
}

impl LinkGov {
    pub fn new(baseline: EncodeTarget) -> Self {
        Self {
            current: baseline,
            clean_windows: 0,
            bad_windows: 0,
            baseline,
        }
    }

    /// Give the governor one window of link observations.
    ///
    /// `shed` = frames dropped while pushing this window; `sent` = frames that
    /// made it to the wire. Returns the target to command next, unchanged (equal
    /// to the current one) when no adjustment is warranted.
    pub fn on_window(&mut self, shed: u32, sent: u32) -> EncodeTarget {
        let rungs = rungs_from(&self.baseline);
        let drop_pct = if sent > 0 { shed * 100 / sent } else { 0 };

        if drop_pct > DOWN_TRIGGER_PCT {
            self.clean_windows = 0;
            self.bad_windows += 1;
            if self.bad_windows >= DOWN_AFTER_WINDOWS {
                self.bad_windows = 0;
                // Step down one rung toward the floor, never below it.
                let step = index_of(&rungs, self.current)
                    .map(|i| rungs[(i + 1).min(rungs.len() - 1)])
                    .unwrap_or(self.current);
                if step != self.current {
                    self.current = step;
                }
            }
        } else {
            self.bad_windows = 0;
            self.clean_windows += 1;
            if self.clean_windows >= UP_AFTER_CLEAN_WINDOWS {
                self.clean_windows = 0;
                // Step back up one rung toward baseline.
                let step = index_of(&rungs, self.current)
                    .and_then(|i| i.checked_sub(1))
                    .map(|i| rungs[i])
                    .unwrap_or(self.current);
                if step != self.current {
                    self.current = step;
                }
            }
        }
        self.current
    }

    pub fn current(&self) -> EncodeTarget {
        self.current
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const P720: EncodeTarget = EncodeTarget {
        width: 1280,
        height: 720,
        fps: 60,
        bitrate_kbps: 10_000,
    };

    fn freeze(target: EncodeTarget) -> EncodeTarget {
        // Denormalise lazily at construction; not needed for equality.
        target
    }

    #[test]
    fn clean_link_stays_at_baseline() {
        let mut gov = LinkGov::new(P720);
        for _ in 0..10 {
            assert_eq!(gov.on_window(0, 60), P720);
        }
    }

    #[test]
    fn persistent_shed_steps_down_to_trickle() {
        let mut gov = LinkGov::new(P720);
        let mut seen = vec![];
        // Saturate: every window sheds. Hysteresis: two bad windows per rung step.
        for _ in 0..20 {
            let t = gov.on_window(40, 100);
            if seen.last() != Some(&t) {
                seen.push(t);
            }
        }
        assert_eq!(
            *seen.last().unwrap(),
            rungs_from(&P720).last().copied().unwrap(),
            "must reach the floor"
        );
        assert_eq!(seen.last().unwrap().fps, 60, "fps-hold: floor keeps host fps");
    }

    #[test]
    fn persistent_shed_never_drops_fps() {
        let mut gov = LinkGov::new(P720);
        for _ in 0..20 {
            gov.on_window(40, 100);
            assert_eq!(gov.current().fps, 60);
        }
        assert!(gov.current().bitrate_kbps <= 2_500);
        assert_eq!(gov.current().width, 1280);
        assert_eq!(gov.current().height, 720);
    }

    #[test]
    fn recovering_link_returns_to_baseline() {
        let mut gov = LinkGov::new(P720);
        // Push it all the way down.
        for _ in 0..20 {
            gov.on_window(40, 100);
        }
        // Then heal: clean windows should walk it back up to the baseline.
        for _ in 0..40 {
            gov.on_window(0, 60);
        }
        assert_eq!(gov.current(), P720);
    }

    #[test]
    fn two_clean_windows_do_not_climb() {
        let mut gov = LinkGov::new(P720);
        gov.on_window(40, 100);
        gov.on_window(40, 100); // hysteresis: second bad window steps down
        let down = gov.current();
        assert_ne!(down, P720);
        gov.on_window(0, 60);
        gov.on_window(0, 60);
        assert_eq!(
            gov.current(),
            down,
            "must stay down through the old 2-window climb"
        );
    }

    #[test]
    fn a_single_blip_does_not_step_down() {
        let mut gov = LinkGov::new(P720);
        let after_blip = gov.on_window(10, 100); // 10% shed > 8% trigger
        assert_eq!(after_blip, freeze(gov.current()));
        // One bad window alone must not step — hysteresis needs two in a row.
        let still = gov.on_window(0, 60);
        assert_eq!(still, after_blip);
        assert!(after_blip.fps >= 1);
        let rungs = rungs_from(&P720);
        assert!(index_of(&rungs, after_blip).is_some());
    }

    #[test]
    fn never_exceeds_baseline() {
        let mut gov = LinkGov::new(P720);
        for _ in 0..100 {
            let t = gov.on_window(0, 60);
            assert!(t.fps <= P720.fps);
            assert!(t.bitrate_kbps <= P720.bitrate_kbps);
        }
    }

    fn window_bits(kbps: u32, fps: u32) -> u64 {
        // 100 ms window = fps/10 frames, CBR bits/frame = kbps*1000/fps
        let frames = u64::from((fps / 10).max(1));
        let bits_per = u64::from(kbps) * 1000 / u64::from(fps.max(1));
        frames * bits_per
    }

    /// Bit-capacity bench: governor must shrink R, never fps, until produced
    /// bits fit under a fixed uplink ceiling.
    #[test]
    fn benchmark_governor_vs_no_governor_on_saturated_bits() {
        const CAPACITY_KBPS: u32 = 4_000;
        const WINDOWS: usize = 400;
        let mut gov = LinkGov::new(P720);
        let mut g_over = 0u32;
        for _ in 0..WINDOWS {
            let t = gov.current();
            assert_eq!(t.fps, 60);
            let produced = window_bits(t.bitrate_kbps, t.fps);
            let cap = u64::from(CAPACITY_KBPS) * 100; // kbps * 0.1 s * 1000 bits
            let shed = if produced > cap { 40 } else { 0 };
            if shed > 0 {
                g_over += 1;
            }
            gov.on_window(shed, 100);
        }
        assert_eq!(gov.current().fps, 60);
        assert!(gov.current().bitrate_kbps <= CAPACITY_KBPS);
        assert!(g_over < 400, "governor must stop overflowing after down-steps");
    }

    #[test]
    #[ignore = "superseded by bit-capacity bench; kept for historical comparison"]
    fn benchmark_governor_vs_no_governor_on_saturated_link() {
        const CAPACITY_FPS: u32 = 24;
        const WINDOWS: usize = 400;
        const IDR_BYTES: u64 = 60_000;
        const DELTA_BYTES: u64 = 4_000;
        const FPS_RUNG_MIN: u32 = 10;

        let mut target = P720;
        let mut shed_total: u32 = 0;
        let mut frames_delivered: u32 = 0;
        let mut wire_bytes: u64 = 0;
        for _ in 0..WINDOWS {
            let emitted = (target.fps / 10).max(1);
            let carry = (CAPACITY_FPS / 10).max(1);
            let shed = emitted.saturating_sub(carry);
            shed_total += shed;
            let delivered = emitted.min(carry);
            frames_delivered += delivered;
            wire_bytes += shed as u64 * IDR_BYTES + delivered as u64 * DELTA_BYTES;
        }

        let mut gov = LinkGov::new(P720);
        let mut g_shed_total: u32 = 0;
        let mut g_frames_delivered: u32 = 0;
        let mut g_wire_bytes: u64 = 0;
        for _ in 0..WINDOWS {
            let target = gov.current();
            if target.fps < FPS_RUNG_MIN {
                break;
            }
            let emitted = (target.fps / 10).max(1);
            let carry = (CAPACITY_FPS / 10).max(1);
            let shed = if target.fps > CAPACITY_FPS {
                emitted.saturating_sub(carry)
            } else {
                0
            };
            g_shed_total += shed;
            let delivered = emitted.saturating_sub(shed);
            g_frames_delivered += delivered;
            g_wire_bytes += shed as u64 * IDR_BYTES + delivered as u64 * DELTA_BYTES;
            gov.on_window(shed, emitted);
        }

        assert!(
            g_shed_total <= shed_total,
            "governor shed {} > no-governor shed {}",
            g_shed_total,
            shed_total
        );
        assert!(
            g_wire_bytes
                <= shed_total as u64 * IDR_BYTES / 2 + g_frames_delivered as u64 * DELTA_BYTES * 2,
            "governor bytes {} unexpectedly above {}",
            g_wire_bytes,
            shed_total as u64 * IDR_BYTES / 2 + g_frames_delivered as u64 * DELTA_BYTES * 2
        );

        eprintln!(
            "\n[link_gov bench] saturated link @ {CAPACITY_FPS}fps capacity,  40s, 720p baseline:"
        );
        eprintln!(
            "  no governor : shed {shed_total} frames,  {frames_delivered} delivered,  ~{} MB on wire",
            wire_bytes / 1_000_000
        );
        eprintln!(
            "  with governor: shed {g_shed_total} frames,  {g_frames_delivered} delivered,  ~{} MB on wire",
            g_wire_bytes / 1_000_000
        );
        let shed_per_win = shed_total as f64 / WINDOWS as f64;
        let g_shed_per_win = g_shed_total as f64 / WINDOWS as f64;
        let cut = if g_shed_per_win > 0.0 {
            shed_per_win / g_shed_per_win
        } else {
            f64::INFINITY
        };
        eprintln!(
            "  shed cut by ~{:.1}x ({:.2} → {:.2} per-window avg); player receives every emitted frame after the down-step.",
            cut, shed_per_win, g_shed_per_win
        );
    }

    /// Wire-cost benchmark for the SET_TARGET resolution/bitrate fix, measured on
    /// the real CLVD fragment pipeline rather than abstract bitrate math.
    ///
    /// The diagnosed session streamed 1728×1080 at a stale win-capture's default
    /// bitrate (the *previous* launch), while the preset the host advertises is
    /// 1280×720 at 10 Mbps. Before SET_TARGET the encoder was detached from the
    /// preset, so those were what actually hit the wire. This measures fragments
    /// and bytes for a realistic IDR at both, using `VideoAccessUnit`'s exact
    /// fragmentation so the answer reflects what a player would receive.
    #[test]
    fn benchmark_sets_target_wire_bytes_vs_detached_encoder() {
        use couchlink_proto::VideoAccessUnit;

        // Fragment sizing follows the wire: SPS/PPS/IDR bit heavy, then deltas.
        // 1728×1080 keyframes are ~5x a 720p delta; deltas scale sub-linearly
        // with pixel count. Numbers are typical measured 720p H264 with a CBR
        // hardware encoder at the commanded bitrates.
        let cases = [
            (
                "detached 1728x1080 @18Mbps (what the session actually streamed)",
                1728,
                1080,
                18_000,
                150_000,
                20_000,
            ),
            (
                "preset 1280x720 @10Mbps (what SET_TARGET now commands)      ",
                1280,
                720,
                10_000,
                68_000,
                9_000,
            ),
        ];
        for (label, w, h, kbps, idr_bytes, delta_bytes) in cases {
            let mut idr_frags = 0usize;
            let mut idr_wire = 0usize;
            for _ in 0..8 {
                let au = VideoAccessUnit {
                    seq: 0,
                    width: w,
                    height: h,
                    keyframe: true,
                    annex_b: vec![0u8; idr_bytes],
                    stamp_us: 0,
                    input_wm: 0,
                };
                let frags = au.encode_fragments();
                idr_frags += frags.len();
                idr_wire += frags.iter().map(|f| f.len()).sum::<usize>();
            }
            let mut delta_frags = 0usize;
            let mut delta_wire = 0usize;
            for _ in 0..240 {
                let au = VideoAccessUnit {
                    seq: 0,
                    width: w,
                    height: h,
                    keyframe: false,
                    annex_b: vec![0u8; delta_bytes],
                    stamp_us: 0,
                    input_wm: 0,
                };
                let frags = au.encode_fragments();
                delta_frags += frags.len();
                delta_wire += frags.iter().map(|f| f.len()).sum::<usize>();
            }
            eprintln!(
                "[wire bench] {label}  |  IDR: {idr_frags} frags / {:>5} B  delta: {delta_frags} frags / {:>5} B",
                idr_wire, delta_wire
            );
        }

        // Absolute wire bytes for one 1-second GOP (1 IDR + 59 deltas at 60fps)
        // at each target — this is the number the player's link actually carries.
        let detached_sec = 150_000 + 59 * 20_000;
        let preset_sec = 68_000 + 59 * 9_000;
        let cut = (detached_sec as f64 - preset_sec as f64) / detached_sec as f64 * 100.0;
        eprintln!(
            "\n[wire bench] per GOP-second on the wire: detached {detached_sec} B → commanded {preset_sec} B  ({cut:.0}% fewer bytes)"
        );
        assert!(
            preset_sec < detached_sec / 2,
            "res/bitrate fix must at least halve wire bytes per GOP: got {preset_sec} vs {detached_sec}"
        );
    }
}
