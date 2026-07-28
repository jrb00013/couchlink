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
  | { type: "answer"; sdp: string }
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
  | {
      type: "stream_info";
      width: number;
      height: number;
      fps: number;
      codec: string;
      capture_ok?: boolean;
      capture_hint?: string;
    };

export function send(ws: WebSocket, msg: SignalMessage) {
  ws.send(JSON.stringify(msg));
}
