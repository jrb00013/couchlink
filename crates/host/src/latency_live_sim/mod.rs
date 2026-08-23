//! Session replay simulators — live-metric regression without a browser.

pub mod death_spiral;
pub mod governor;
pub mod paint;
pub mod ricardo_gate;

pub use death_spiral::{simulate_governor_drop_pct, simulate_two_peer_shed_counting};
pub use governor::{simulate_governor_session, GovernorSessionResult};
pub use paint::{simulate_paint_fps, PaintSimConfig};
pub use ricardo_gate::{beats_ricardo, SessionMetrics, RICARDO};
