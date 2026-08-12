/** Mirrors crates/proto — signaling for couchlink co-play */

export type Role = "host" | "player";

export type SignalMessage =
  | {
      type: "register_host";
      session_id: string;
      pin: string;
      device_name?: string;
      preset?: string;
      emulator?: string;
    }
  | {
      type: "register_player";
      session_id: string;
      pin: string;
      player_name?: string;
    }
  | { type: "registered"; role: Role; session_id: string }
  | { type: "error"; message: string }
  | { type: "offer"; sdp: string; epoch?: number }
  | { type: "answer"; sdp: string; epoch?: number }
  | {
      type: "ice_candidate";
      candidate: string;
      sdpMid?: string | null;
      sdpMLineIndex?: number | null;
    }
  | { type: "heartbeat" }
  | { type: "pong" }
  | { type: "request_offer" }
  | { type: "peer_joined"; role: Role; epoch?: number }
  | { type: "peer_left" }
  /** Player → host: which controller family this pad is, so the host can match
   * the virtual device and emulator binding. The Gamepad API normalises input,
   * so the host cannot tell an Xbox pad from a DualSense without being told. */
  | { type: "pad_info"; kind: string; id: string }
  /** Player → host: which video path it is actually presenting from, so the
   * host can stop writing the path nobody is painting from. */
  | { type: "present_path"; path: "webcodecs" | "rtp" }
  | {
      type: "stream_info";
      width: number;
      height: number;
      fps: number;
      codec: string;
      capture_ok?: boolean;
      capture_hint?: string;
    }
  | {
      type: "host_stats";
      fps: number;
      frames_out: number;
      dropped_frames: number;
      drop_pct: number;
      capture_ms: number;
      scale_ms: number;
      encode_ms: number;
      push_ms: number;
      dominant_stage: string;
      target_width: number;
      target_height: number;
      target_fps: number;
      target_bitrate_kbps: number;
    };

export function send(ws: WebSocket, msg: SignalMessage) {
  ws.send(JSON.stringify(msg));
}
