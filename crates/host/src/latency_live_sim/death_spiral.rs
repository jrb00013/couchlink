//! Two-peer join_all fate arithmetic — the bug that pinned shed% at ~50%.

use crate::webrtc_peer::{governor_shed_counts, PushFate};

/// Count shed the **old** way: every non-delivery (including TrickleSkip) was a shed.
pub fn legacy_shed_count(fates: &[PushFate]) -> u64 {
    fates
        .iter()
        .filter(|f| **f != PushFate::Delivered)
        .count() as u64
}

/// Count shed the **new** way: only real congestion sheds (`PushFate::Shed`).
pub fn current_shed_count(fates: &[PushFate]) -> u64 {
    governor_shed_counts(fates).1
}

/// One cadence tick, two peers: healthy + slow trickle.
pub fn simulate_two_peer_shed_counting(count_trickle_as_shed: bool) -> u32 {
    let fates = [PushFate::Delivered, PushFate::TrickleSkip];
    let shed = if count_trickle_as_shed {
        legacy_shed_count(&fates)
    } else {
        current_shed_count(&fates)
    };
    let sent = fates.len() as u32;
    if sent == 0 {
        return 0;
    }
    shed as u32 * 100 / sent
}

/// Governor drop% from a window of per-peer fates (one fate per pushed frame).
pub fn simulate_governor_drop_pct(fates: &[PushFate]) -> u32 {
    let shed = current_shed_count(fates) as u32;
    let sent = fates.len() as u32;
    if sent == 0 {
        return 0;
    }
    shed * 100 / sent
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legacy_two_peer_trickle_reports_fifty_pct_shed() {
        assert_eq!(simulate_two_peer_shed_counting(true), 50);
    }

    #[test]
    fn fixed_two_peer_trickle_reports_zero_pct_shed() {
        assert_eq!(simulate_two_peer_shed_counting(false), 0);
    }

    #[test]
    fn real_shed_still_counts_for_governor() {
        let fates = [PushFate::Delivered, PushFate::Shed];
        assert_eq!(simulate_governor_drop_pct(&fates), 50);
    }

    #[test]
    fn three_peer_one_slow_trickle_stays_under_governor_trigger() {
        let fates = [
            PushFate::Delivered,
            PushFate::Delivered,
            PushFate::TrickleSkip,
        ];
        assert!(simulate_governor_drop_pct(&fates) < 8);
    }
}
