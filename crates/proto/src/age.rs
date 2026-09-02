//! Glass-to-glass age: host stamp → client paint echo → host p50/p95.
//!
//! Clock offset cancels because the client echoes the same `stamp_us` and the
//! host subtracts it from *its* now. `stamp_us == 0` means "unknown" (v2).

use serde::{Deserialize, Serialize};

/// Player → host on the pad DataChannel (JSON). Never a `PadFrame`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub struct AgeEcho {
    pub seq: u32,
    pub stamp_us: u64,
    pub recv_ms: f64,
    pub paint_ms: f64,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum PadInboundJson {
    AgeEcho {
        seq: u32,
        stamp_us: u64,
        recv_ms: f64,
        paint_ms: f64,
    },
    #[serde(other)]
    Other,
}

pub fn parse_age_echo_json(text: &str) -> Option<AgeEcho> {
    match serde_json::from_str::<PadInboundJson>(text).ok()? {
        PadInboundJson::AgeEcho {
            seq,
            stamp_us,
            recv_ms,
            paint_ms,
        } => Some(AgeEcho {
            seq,
            stamp_us,
            recv_ms,
            paint_ms,
        }),
        PadInboundJson::Other => None,
    }
}

pub fn age_ms(now_us: u64, stamp_us: u64) -> f64 {
    if stamp_us == 0 {
        return 0.0;
    }
    now_us.saturating_sub(stamp_us) as f64 / 1000.0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn age_echo_json_round_trips() {
        let raw = r#"{"type":"age_echo","seq":1,"stamp_us":9,"recv_ms":1.0,"paint_ms":2.0}"#;
        let e = parse_age_echo_json(raw).expect("age_echo");
        assert_eq!(e.seq, 1);
        assert_eq!(e.stamp_us, 9);
        assert!((e.paint_ms - e.recv_ms - 1.0).abs() < 1e-9);
    }

    #[test]
    fn rumble_json_is_not_an_age_echo() {
        assert!(parse_age_echo_json(r#"{"type":"rumble","large":1,"small":2}"#).is_none());
    }

    #[test]
    fn zero_stamp_is_unknown_age() {
        assert_eq!(age_ms(50_000, 0), 0.0);
        assert!((age_ms(50_000, 40_000) - 10.0).abs() < 1e-9);
    }
}
