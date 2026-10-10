//! What the capture encoder does when the host commands a new (fps, bitrate) target.
//!
//! Kept free of any Windows/COM types so the policy is unit-tested on every platform. History: until 2026-10-03 every
//! target change - including the link governor's routine bitrate steps - tore the hardware encoder down and rebuilt it. A rebuild
//! is a gap with no frames, a fresh IDR and a latency spike, and at 4000 kbps the NVIDIA MFT rejected one ("input type is not
//! supported for D3D device"): no encoder, no frames, the host's 1.5 s stale-frame rule dropped the capture, it reconnected, the host
//! commanded the same target and the rebuild failed again - a permanent reconnect loop with 0 fps sent.

/// A (fps, bitrate in bits/s) pair.
pub type Target = (u32, u32);

#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum TargetPlan {
    /// Nothing to do: the running encoder already matches, or this exact target was already rejected by a build.
    Keep,
    /// Same fps, different bitrate: change the rate on the running encoder. No teardown, no gap, no extra latency.
    ApplyBitrate(u32),
    /// fps changed (the MFT is bound to one frame rate at build time): rebuild.
    Rebuild,
}

/// Decide what to do. `current` is what the live encoder was built/updated to, `wanted` is the host's latest command and
/// `rejected` is the last target a build refused (never retried until the host commands something different).
pub fn plan_target_change(current: Target, wanted: Target, rejected: Option<Target>) -> TargetPlan {
    if wanted == current || rejected == Some(wanted) {
        return TargetPlan::Keep;
    }
    if wanted.0 == current.0 {
        TargetPlan::ApplyBitrate(wanted.1)
    } else {
        TargetPlan::Rebuild
    }
}

/// After a failed rebuild: which known-good target to rebuild with instead, if any. None means "no fallback available" (the first
/// build ever failed, or the failed target IS the last good one) and the caller keeps today's behaviour.
pub fn fallback_after_failed_build(attempted: Target, last_good: Option<Target>) -> Option<Target> {
    last_good.filter(|good| *good != attempted)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matching_target_keeps_the_encoder() {
        assert_eq!(plan_target_change((60, 10_000_000), (60, 10_000_000), None), TargetPlan::Keep);
    }

    #[test]
    fn governor_bitrate_steps_never_rebuild() {
        // 10000 -> 8000 -> 6000 -> 4000 kbps, the exact sequence that froze the stream on 2026-10-03.
        let mut cur = (60, 10_000_000);
        for kbps in [8000, 6000, 4000] {
            let plan = plan_target_change(cur, (60, kbps * 1000), None);
            assert_eq!(plan, TargetPlan::ApplyBitrate(kbps * 1000));
            cur = (60, kbps * 1000);
        }
    }

    #[test]
    fn bitrate_going_back_up_also_applies_in_place() {
        assert_eq!(plan_target_change((60, 4_000_000), (60, 10_000_000), None), TargetPlan::ApplyBitrate(10_000_000));
    }

    #[test]
    fn fps_change_rebuilds() {
        assert_eq!(plan_target_change((60, 10_000_000), (30, 10_000_000), None), TargetPlan::Rebuild);
        assert_eq!(plan_target_change((60, 10_000_000), (30, 4_000_000), None), TargetPlan::Rebuild);
    }

    #[test]
    fn a_rejected_target_is_not_retried_every_frame() {
        let rejected = Some((30, 4_000_000));
        assert_eq!(plan_target_change((60, 10_000_000), (30, 4_000_000), rejected), TargetPlan::Keep);
        // ...but a different command is honoured again.
        assert_eq!(plan_target_change((60, 10_000_000), (60, 8_000_000), rejected), TargetPlan::ApplyBitrate(8_000_000));
    }

    #[test]
    fn failed_rebuild_falls_back_to_last_good() {
        assert_eq!(fallback_after_failed_build((30, 4_000_000), Some((60, 10_000_000))), Some((60, 10_000_000)));
    }

    #[test]
    fn no_fallback_when_nothing_worked_before_or_it_is_the_same_target() {
        assert_eq!(fallback_after_failed_build((60, 10_000_000), None), None);
        assert_eq!(fallback_after_failed_build((60, 10_000_000), Some((60, 10_000_000))), None);
    }
}
