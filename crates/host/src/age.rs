//! Host clock for CLVD stamps and AgeEcho percentiles.

use std::sync::{Mutex, OnceLock};
use std::time::Instant;

use couchlink_proto::age_ms;

static ORIGIN: OnceLock<Instant> = OnceLock::new();

fn origin() -> Instant {
    *ORIGIN.get_or_init(Instant::now)
}

/// Microseconds since host start. Same domain the client echoes back.
pub fn now_us() -> u64 {
    origin().elapsed().as_micros() as u64
}

/// Ring of recent glass-to-glass ages (ms).
#[derive(Debug, Default)]
pub struct AgeStats {
    samples: Vec<f64>,
    cap: usize,
}

impl AgeStats {
    pub fn new(cap: usize) -> Self {
        Self {
            samples: Vec::with_capacity(cap.min(256)),
            cap: cap.max(8),
        }
    }

    pub fn record(&mut self, age_ms: f64) {
        if !age_ms.is_finite() || age_ms <= 0.0 {
            return;
        }
        if self.samples.len() == self.cap {
            self.samples.remove(0);
        }
        self.samples.push(age_ms);
    }

    /// (p50, p95) in ms. (0, 0) until we have a sample.
    pub fn percentiles(&self) -> (f64, f64) {
        if self.samples.is_empty() {
            return (0.0, 0.0);
        }
        let mut v = self.samples.clone();
        v.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        let p = |q: f64| {
            let i = ((v.len() as f64 - 1.0) * q).round() as usize;
            v[i.min(v.len() - 1)]
        };
        (p(0.50), p(0.95))
    }
}

pub fn echo_age_ms(echo_stamp_us: u64) -> f64 {
    age_ms(now_us(), echo_stamp_us)
}

fn global() -> &'static Mutex<AgeStats> {
    static G: OnceLock<Mutex<AgeStats>> = OnceLock::new();
    G.get_or_init(|| Mutex::new(AgeStats::new(64)))
}

pub fn record_global(age_ms: f64) {
    if let Ok(mut g) = global().lock() {
        g.record(age_ms);
    }
}

pub fn global_percentiles() -> (f64, f64) {
    global()
        .lock()
        .map(|g| g.percentiles())
        .unwrap_or((0.0, 0.0))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn percentiles_on_known_ring() {
        let mut s = AgeStats::new(16);
        for a in [10.0, 20.0, 30.0, 40.0, 50.0] {
            s.record(a);
        }
        let (p50, p95) = s.percentiles();
        assert!((p50 - 30.0).abs() < 1e-9);
        assert!((p95 - 50.0).abs() < 1e-9);
    }

    #[test]
    fn zero_and_nan_are_ignored() {
        let mut s = AgeStats::new(8);
        s.record(0.0);
        s.record(f64::NAN);
        assert_eq!(s.percentiles(), (0.0, 0.0));
    }
}
