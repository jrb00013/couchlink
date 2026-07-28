//! Couchlink wire protocol shared by signaling, host, and client.
//!
//! Methodologies mirror Rohomieo: tagged JSON `type` discriminators in snake_case
//! for WebSocket signaling; media stays peer-to-peer. Pad state uses a compact
//! binary frame (`CLPD`) on the WebRTC DataChannel named `pad`.

pub mod pad_frame;
pub mod signal;

pub use pad_frame::{PadFeedback, PadFrame, PAD_CHANNEL, PAD_MAGIC};
pub use signal::{Role, SignalMessage, StreamPreset};
