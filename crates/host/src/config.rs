use clap::Parser;
use couchlink_proto::StreamPreset;

#[derive(Parser, Debug, Clone)]
#[command(name = "couchlink-host", about = "Host co-play session for emulators", version)]
pub struct HostArgs {
    #[arg(long, env = "COUCHLINK_SIGNALING", default_value = "ws://127.0.0.1:8443/ws")]
    pub signaling: String,
    #[arg(long, env = "COUCHLINK_SESSION_ID")]
    pub session_id: String,
    #[arg(long, env = "COUCHLINK_PIN")]
    pub pin: String,
    #[arg(long, default_value = "couchlink-host")]
    pub device_name: String,
    #[arg(long, default_value = "1080p60", env = "COUCHLINK_PRESET")]
    pub preset: String,
    #[arg(long, default_value = "auto")]
    pub emulator: String,
    /// Idle FPS when motion detector sees a still frame (Rohomieo method).
    #[arg(long, default_value = "8")]
    pub idle_fps: u32,
    #[arg(long, default_value_t = true)]
    pub bluetooth_pad: bool,
}

impl HostArgs {
    pub fn stream_preset(&self) -> StreamPreset {
        StreamPreset::parse(&self.preset).unwrap_or(StreamPreset::P1080_60)
    }
}
