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
    /// Local coturn relay, e.g. turn:1.2.3.4:3478 (see scripts/start-turn.sh)
    #[arg(long, env = "COUCHLINK_TURN_URL")]
    pub turn_url: Option<String>,
    #[arg(long, env = "COUCHLINK_TURN_USER")]
    pub turn_user: Option<String>,
    #[arg(long, env = "COUCHLINK_TURN_PASS")]
    pub turn_pass: Option<String>,
    /// Advertise these host IPs in ICE (WSL: set to `hostname -I` LAN address, not Docker bridges).
    #[arg(long, env = "COUCHLINK_ICE_IPS", value_delimiter = ',')]
    pub ice_ips: Vec<String>,
}

impl HostArgs {
    pub fn stream_preset(&self) -> StreamPreset {
        StreamPreset::parse(&self.preset).unwrap_or(StreamPreset::P1080_60)
    }
}
