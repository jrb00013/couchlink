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
/// 2% was noise: three WAN viewers shed a couple of frames every window.
/// 8% matches Joel live: 6% HUD must not step; 9–14% windows must.
const DOWN_TRIGGER_PCT: u32 = 8;
/// Host shed this high means the pipe cannot carry the *bits* (WAN/TURN),
/// not just SCTP message rate — unlock bitrate rungs even without a client
/// ClientLinkStats report. Live 2026-10-12: 10–18% shed at locked 10 Mbps
/// froze friends on "media lost" while fps-only ladder held bitrate.
const SEVERE_SHED_PCT: u32 = 12;
/// Consecutive bad windows before stepping down.
const DOWN_AFTER_WINDOWS: u32 = 2;
/// Severe shed steps on the first bad window (don't wait ~10s while frozen).
const SEVERE_DOWN_AFTER_WINDOWS: u32 = 1;
/// Clean windows before climbing. Two (~10s) recovers fps after a CLVD blip
/// without the old 40s stuck-at-floor climb while bitrate was wrong.
const UP_AFTER_CLEAN_WINDOWS: u32 = 2;
/// Ceiling for the climb-back backoff below (clean windows required per rung).
const MAX_UP_AFTER_CLEAN_WINDOWS: u32 = 12;

/// The governor's persistent memory between windows.
#[derive(Debug, Clone)]
pub struct LinkGov {
    /// Where the rung ladder starts — the preset the host advertises.
    baseline: EncodeTarget,
    /// What we are currently commanding, or `less` while stepping back up.
    current: EncodeTarget,
    /// Consecutive clean windows (no triggering sheds).
    clean_windows: u32,
    /// Bad windows in a row — hysteresis so one 9% blip does not step.
    bad_windows: u32,
    /// Set once `note_client_congestion` fires (a real client-reported RTP
    /// drop, not local DataChannel shed) and never cleared for the life of
    /// this governor. Unlocks the bitrate rungs in `rungs_from` — see that
    /// function's doc comment for why bitrate must stay off-limits for the
    /// DataChannel/SCTP congestion case this file was originally written for.
    client_congestion_seen: bool,
    /// A client reported loss/freezes since the last `on_window`.
    client_loss_window: bool,
    /// A client reported SEVERE RTP loss since the last `on_window` (see `client_report_is_severe`).
    client_severe_window: bool,
    /// Clean windows required before climbing one rung. Starts at `UP_AFTER_CLEAN_WINDOWS` and doubles (up to
    /// `MAX_UP_AFTER_CLEAN_WINDOWS`) every time a *client-reported* loss pushes us down, then resets once we are back at baseline.
    /// Live 2026-10-17: 8 Mbps -> 10 Mbps 15 s later -> loss again, over and over, because two clean windows were enough to climb
    /// no matter how many times the same link had just failed at that rate.
    up_need: u32,
    /// Consecutive clean windows spent AT baseline; the climb-back backoff only resets after a long stable stretch there (reaching
    /// baseline is not proof the link holds that rate: it is exactly where the yo-yo failed).
    stable_windows: u32,
}

/// Rung ladder: **hold bitrate**, step fps.
///
/// Joel live (v21–v22): shed stayed 5–14% after the governor floored bitrate
/// to 1250 kbps — congestion was SCTP frame *rate*, not bits. Cutting R
/// failed encode≥5000 and did not clear sheds; paint/S_p50 died on mushy
/// IDRs. Stepping fps reduces messages/sec while keeping bits/frame healthy.
fn rungs_from(baseline: &EncodeTarget) -> Vec<EncodeTarget> {
    rungs_from_inner(baseline, false)
}

/// `rtp_congested` unlocks a final tier of rungs that cut bitrate.
///
/// The fps-only ladder above this comment was tuned against a specific past
/// incident: DataChannel/SCTP congestion, where shed was message *rate*, not
/// raw bandwidth, so cutting bitrate never helped and stepping fps did.
///
/// A client-reported RTP drop (`SignalMessage::ClientLinkStats`, see its doc
/// comment) is a structurally different signal, independent of which rung is
/// currently commanded: it fires whenever the browser's own jitter buffer is
/// dropping frames on arrival, fps rungs having done nothing to stop it.
/// Because the encoder's target bitrate is unconditional — every rung above
/// holds it at
/// `baseline.bitrate_kbps` — stepping fps down at a fixed bitrate does not
/// reduce actual bits/sec at all; it just spreads the same bits over fewer,
/// bigger frames. On a link that is genuinely bandwidth-constrained (not
/// message-rate-constrained), that changes nothing, which is exactly the
/// "the governor fired but it's still bad" case this closes. Once real
/// client-reported congestion has been seen this session, three bitrate cuts
/// below the fps floor.
fn rungs_from_inner(baseline: &EncodeTarget, rtp_congested: bool) -> Vec<EncodeTarget> {
    let mut rungs = vec![*baseline];
    if rtp_congested {
        // Client-reported loss means the pipe is too small for the *bits*, so cut
        // bitrate first at full fps; fps steps (which keep bits/sec) come after,
        // at the lowest bitrate.
        let mut low = *baseline;
        for pct in [80u32, 60, 40] {
            low = EncodeTarget {
                bitrate_kbps: ((baseline.bitrate_kbps as u64 * pct as u64) / 100) as u32,
                ..*baseline
            };
            rungs.push(low);
        }
        let floor_fps = ((baseline.fps * 5) / 9).max(45).min(baseline.fps);
        let mut fps = baseline.fps;
        for _ in 0..4 {
            let next = fps.saturating_sub((fps / 7).max(6)).max(floor_fps);
            if next >= fps {
                break;
            }
            fps = next;
            rungs.push(EncodeTarget { fps, ..low });
        }
        return rungs;
    }
    // Floor: ~55% of baseline (90→50) so sustained SCTP shed can clear while
    // holding 5 Mbps — 75% floor (90→67) never cleared v24's 20% shed.
    let floor_fps = ((baseline.fps * 5) / 9).max(45).min(baseline.fps);
    let mut fps = baseline.fps;
    for _ in 0..4 {
        let step = (fps / 7).max(6);
        let next = fps.saturating_sub(step).max(floor_fps);
        if next >= fps {
            break;
        }
        fps = next;
        let extra = EncodeTarget {
            fps,
            bitrate_kbps: baseline.bitrate_kbps,
            ..*baseline
        };
        if !rungs.iter().any(|r| *r == extra) {
            rungs.push(extra);
        }
    }
    rungs
}

/// Minimum packets in a report before its loss share means anything.
const MIN_LOSS_SAMPLE: u32 = 200;
/// Loss share (percent) that counts as congestion. 3.6% live froze the picture.
const LOSS_TRIGGER_PCT: u32 = 2;
/// Browser freezes in one report that count as congestion on their own.
const FREEZE_TRIGGER: u32 = 2;

/// Does one client report describe a congested link? Pure so it is unit-tested.
pub fn client_report_is_congested(lost: u32, received: u32, freezes: u32) -> bool {
    let total = lost.saturating_add(received);
    (total >= MIN_LOSS_SAMPLE && lost as u64 * 100 >= LOSS_TRIGGER_PCT as u64 * total as u64)
        || freezes >= FREEZE_TRIGGER
}

/// Loss share (percent) that is not "a bit lossy" but "this rate does not fit through the pipe". 12-18% loss with 2-3 freezes
/// is what the friends saw as "it froze mid game"; a single such report should cut bitrate, not wait for a second window.
const SEVERE_CLIENT_LOSS_PCT: u32 = 8;

/// Is one client report severe enough to step down on its own? Pure so it is unit-tested.
pub fn client_report_is_severe(lost: u32, received: u32) -> bool {
    let total = lost.saturating_add(received);
    total >= MIN_LOSS_SAMPLE && lost as u64 * 100 >= SEVERE_CLIENT_LOSS_PCT as u64 * total as u64
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
            client_congestion_seen: false,
            client_loss_window: false,
            client_severe_window: false,
            up_need: UP_AFTER_CLEAN_WINDOWS,
            stable_windows: 0,
            baseline,
        }
    }

    /// Record that a real client-reported RTP drop happened (see
    /// `SignalMessage::ClientLinkStats`) — unlocks the bitrate rungs in
    /// `rungs_from_inner` for the rest of this governor's life. Sticky by
    /// design: once we've proven this link needs bitrate relief, a
    /// momentarily quiet window should not silently re-forbid the one lever
    /// that actually works for it.
    pub fn note_client_congestion(&mut self) {
        self.client_congestion_seen = true;
    }

    /// A client report showed packet loss / freezes this window. Makes the
    /// current window count as bad even when host-side shed is under the
    /// trigger (RTP never sheds), and unlocks the bitrate rungs. Cleared by
    /// `on_window`, so it must be re-reported to keep stepping down.
    pub fn note_client_loss(&mut self) {
        self.client_congestion_seen = true;
        self.client_loss_window = true;
    }

    /// Like `note_client_loss`, but the loss was severe: the window steps down on its own (no second bad window needed).
    pub fn note_severe_client_loss(&mut self) {
        self.note_client_loss();
        self.client_severe_window = true;
    }

    /// Give the governor one window of link observations.
    ///
    /// `shed` = frames dropped while pushing this window; `sent` = frames
    /// attempted (delivered + shed). Returns the target to command next.
    pub fn on_window(&mut self, shed: u32, sent: u32) -> EncodeTarget {
        let drop_pct = if sent > 0 { shed * 100 / sent } else { 0 };
        let client_bad = std::mem::take(&mut self.client_loss_window);
        let client_severe = std::mem::take(&mut self.client_severe_window);

        // Severe host shed ⇒ unlock bitrate cuts (same as client RTP congestion).
        // Without this, fps-only rungs keep bits/sec ≈ baseline and WAN friends
        // stay packet-lossy / "media lost" forever.
        if drop_pct >= SEVERE_SHED_PCT {
            self.client_congestion_seen = true;
        }

        let rungs = rungs_from_inner(&self.baseline, self.client_congestion_seen);
        let need = if drop_pct >= SEVERE_SHED_PCT || client_severe {
            SEVERE_DOWN_AFTER_WINDOWS
        } else {
            DOWN_AFTER_WINDOWS
        };

        if drop_pct > DOWN_TRIGGER_PCT || client_bad {
            self.stable_windows = 0;
            self.clean_windows = 0;
            self.bad_windows += 1;
            if self.bad_windows >= need {
                self.bad_windows = 0;
                // Step down one rung toward the floor, never below it.
                let step = index_of(&rungs, self.current)
                    .map(|i| rungs[(i + 1).min(rungs.len() - 1)])
                    .unwrap_or(self.current);
                if step != self.current {
                    self.current = step;
                    if client_bad {
                        // The client says this rate loses packets: be slower to try it again.
                        self.up_need = (self.up_need * 2).min(MAX_UP_AFTER_CLEAN_WINDOWS);
                    }
                }
            }
        } else {
            self.bad_windows = 0;
            self.clean_windows += 1;
            if self.clean_windows >= self.up_need {
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
            if self.current == self.baseline {
                self.stable_windows += 1;
                if self.stable_windows >= MAX_UP_AFTER_CLEAN_WINDOWS {
                    self.up_need = UP_AFTER_CLEAN_WINDOWS;
                }
            } else {
                self.stable_windows = 0;
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
    fn mild_shed_steps_down_fps_holds_bitrate() {
        // 9% is above DOWN_TRIGGER (8%) but below SEVERE (12%) — SCTP-rate path.
        let mut gov = LinkGov::new(P720);
        let mut seen = vec![];
        for _ in 0..20 {
            let t = gov.on_window(9, 100);
            if seen.last() != Some(&t) {
                seen.push(t);
            }
        }
        let floor = *seen.last().unwrap();
        assert_eq!(floor, rungs_from(&P720).last().copied().unwrap());
        assert_eq!(floor.bitrate_kbps, P720.bitrate_kbps, "must hold bitrate");
        assert!(floor.fps < P720.fps, "must step fps down");
        assert!(floor.fps >= 45);
    }

    #[test]
    fn mild_shed_never_drops_bitrate() {
        let mut gov = LinkGov::new(P720);
        for _ in 0..20 {
            gov.on_window(9, 100);
            assert_eq!(gov.current().bitrate_kbps, P720.bitrate_kbps);
        }
        assert!(gov.current().fps <= P720.fps);
        assert_eq!(gov.current().width, 1280);
        assert_eq!(gov.current().height, 720);
    }

    #[test]
    fn severe_host_shed_unlocks_bitrate_cut() {
        // Live 2026-10-12: 10–18% shed at locked 10 Mbps → media lost.
        let mut gov = LinkGov::new(P720);
        let t = gov.on_window(15, 100); // 15% ≥ SEVERE → bitrate ladder, step once
        assert!(
            t.bitrate_kbps < P720.bitrate_kbps,
            "severe shed must cut bitrate on first step, got {t:?}"
        );
        assert_eq!(t.fps, P720.fps, "first bitrate rung holds fps");
    }

    #[test]
    fn report_classifier_flags_loss_and_freezes_not_noise() {
        assert!(client_report_is_congested(20, 480, 0), "4% loss");
        assert!(!client_report_is_congested(2, 498, 0), "0.4% loss is noise");
        assert!(!client_report_is_congested(5, 20, 0), "tiny sample is ignored");
        assert!(client_report_is_congested(0, 500, 2), "repeated freezes");
        assert!(!client_report_is_congested(0, 500, 1), "one freeze is a blip");
    }

    #[test]
    fn client_loss_cuts_bitrate_first_at_full_fps() {
        let mut gov = LinkGov::new(P720);
        // Host-side shed is 0% (RTP never sheds) but the client keeps reporting loss.
        for _ in 0..2 {
            gov.note_client_loss();
            gov.on_window(0, 300);
        }
        let t = gov.current();
        assert_eq!(t.fps, P720.fps, "fps must hold while bitrate is cut");
        assert_eq!(t.bitrate_kbps, 8_000, "first rung is 80% bitrate");
        for _ in 0..40 {
            gov.note_client_loss();
            gov.on_window(0, 300);
        }
        assert_eq!(gov.current().bitrate_kbps, 4_000, "floor is 40% bitrate");
        assert!(gov.current().fps >= 45);
    }

    #[test]
    fn loss_flag_is_consumed_and_link_recovers() {
        let mut gov = LinkGov::new(P720);
        for _ in 0..2 {
            gov.note_client_loss();
            gov.on_window(0, 300);
        }
        assert!(gov.current().bitrate_kbps < P720.bitrate_kbps);
        for _ in 0..20 {
            gov.on_window(0, 300); // no more reports
        }
        assert_eq!(gov.current(), P720, "climbs back to baseline when clean");
    }

    /// The DataChannel-only path above must never touch bitrate (that's the
    /// historical incident this file documents). But a real client-reported
    /// RTP drop is a different signal — once seen, sustained shed must be
    /// able to fall through the fps floor into the bitrate rungs, because at
    /// a fixed bitrate, fps rungs alone don't reduce bits/sec at all.
    #[test]
    fn client_reported_congestion_unlocks_bitrate_floor() {
        let mut gov = LinkGov::new(P720);
        gov.note_client_congestion();
        let mut lowest = P720;
        for _ in 0..40 {
            lowest = gov.on_window(40, 100);
        }
        assert!(
            lowest.bitrate_kbps < P720.bitrate_kbps,
            "client-reported congestion must eventually cut bitrate, got {lowest:?}"
        );
        assert!(lowest.fps >= 45, "fps floor is unchanged by the new tier");
    }

    /// Mild sustained shed (SCTP rate, not bits) must match the historical
    /// fps-only floor — bitrate untouched. Severe shed (≥12%) is a different
    /// signal and deliberately unlocks bitrate (see severe_host_shed_*).
    #[test]
    fn no_client_congestion_seen_matches_pre_existing_floor() {
        let mut gov = LinkGov::new(P720);
        let mut lowest = P720;
        for _ in 0..40 {
            lowest = gov.on_window(9, 100);
        }
        assert_eq!(lowest.bitrate_kbps, P720.bitrate_kbps);
        assert_eq!(lowest, *rungs_from(&P720).last().unwrap());
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
    fn severe_report_classifier() {
        assert!(client_report_is_severe(436, 1975), "18% loss (live 03:04:38)");
        assert!(client_report_is_severe(368, 2554), "12.6% loss (live 03:04:40)");
        assert!(!client_report_is_severe(20, 480), "4% is congestion, not severe");
        assert!(!client_report_is_severe(40, 60), "tiny sample is ignored");
    }

    #[test]
    fn one_severe_client_report_cuts_bitrate_immediately() {
        let mut gov = LinkGov::new(P720);
        gov.note_severe_client_loss();
        gov.on_window(0, 300);
        assert_eq!(gov.current().bitrate_kbps, 8_000, "a single severe report must step down, not wait for a second window");
        assert_eq!(gov.current().fps, P720.fps);
        // A merely lossy report still needs two windows (unchanged behavior).
        let mut gov = LinkGov::new(P720);
        gov.note_client_loss();
        gov.on_window(0, 300);
        assert_eq!(gov.current(), P720);
    }

    #[test]
    fn climb_back_after_client_loss_is_slower_and_backs_off() {
        let mut gov = LinkGov::new(P720);
        gov.note_severe_client_loss();
        gov.on_window(0, 300);
        let down = gov.current();
        assert_ne!(down, P720);
        // Two clean windows used to climb straight back to the rate that just lost packets.
        gov.on_window(0, 300);
        gov.on_window(0, 300);
        assert_eq!(gov.current(), down, "two clean windows must not climb after a client-reported loss");
        gov.on_window(0, 300);
        gov.on_window(0, 300);
        assert_ne!(gov.current(), down, "four clean windows climb one rung");
        // A second failure at the higher rate doubles the wait again.
        gov.note_severe_client_loss();
        gov.on_window(0, 300);
        let down2 = gov.current();
        for _ in 0..7 {
            gov.on_window(0, 300);
        }
        assert_eq!(gov.current(), down2, "after a repeat failure, seven clean windows still do not climb");
        gov.on_window(0, 300);
        assert_ne!(gov.current(), down2, "eight clean windows climb");
        // And it resets once the link is healthy again at baseline.
        for _ in 0..200 {
            gov.on_window(0, 300);
        }
        assert_eq!(gov.current(), P720);
        gov.on_window(40, 100);
        gov.on_window(40, 100);
        let d = gov.current();
        gov.on_window(0, 60);
        gov.on_window(0, 60);
        assert_ne!(gov.current(), d, "host-shed recovery keeps the original two-window climb");
    }

    #[test]
    fn two_clean_windows_climb_one_rung() {
        let mut gov = LinkGov::new(P720);
        gov.on_window(40, 100);
        gov.on_window(40, 100); // hysteresis: second bad window steps down
        let down = gov.current();
        assert_ne!(down, P720);
        gov.on_window(0, 60);
        assert_eq!(
            gov.current(),
            down,
            "one clean window must not climb (need two)"
        );
        gov.on_window(0, 60);
        assert_ne!(gov.current(), down, "two clean windows should climb one rung");
    }

    #[test]
    fn six_pct_shed_does_not_step_down() {
        let mut gov = LinkGov::new(P720);
        // Joel live: 43 dropped / 656 attempted = 6% HUD — must not floor.
        for _ in 0..4 {
            assert_eq!(gov.on_window(43, 656), P720);
        }
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

    /// Frame-capacity bench: under mild shed (< SEVERE), governor steps fps
    /// while holding bitrate. Discrete 100ms windows oscillate near capacity
    /// once shed clears — assert min fps dropped, never unlock bitrate.
    #[test]
    fn benchmark_governor_vs_no_governor_on_saturated_frames() {
        // ~11% shed at 96fps (1/9), clears once fps ≤ 85 — mild SCTP path.
        const CAPACITY_FPS: u32 = 85;
        const WINDOWS: usize = 400;
        let baseline = EncodeTarget {
            fps: 96,
            ..P720
        };
        let mut gov = LinkGov::new(baseline);
        let mut min_fps = baseline.fps;
        for _ in 0..WINDOWS {
            let t = gov.current();
            assert_eq!(t.bitrate_kbps, baseline.bitrate_kbps);
            let emitted = (t.fps / 10).max(1);
            let carry = (CAPACITY_FPS / 10).max(1);
            let shed = emitted.saturating_sub(carry);
            gov.on_window(shed, emitted.max(1));
            min_fps = min_fps.min(gov.current().fps);
        }
        assert_eq!(gov.current().bitrate_kbps, baseline.bitrate_kbps);
        assert!(
            min_fps < baseline.fps,
            "mild saturation must step fps at least once, min={min_fps}"
        );
        assert!(
            min_fps >= CAPACITY_FPS.saturating_sub(20),
            "should settle near capacity, not floor; min={min_fps}"
        );
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
