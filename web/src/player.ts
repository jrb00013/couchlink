import { encodeClpd, fromBrowserGamepad, PAD_CHANNEL, type PadState } from "./clpd";
import { send, type SignalMessage } from "./proto";

export type ConnectionState =
  | "disconnected"
  | "connecting"
  | "registering"
  | "waiting_host"
  | "negotiating"
  | "connected"
  | "error";

export interface PlayerCallbacks {
  onState: (s: ConnectionState, detail?: string) => void;
  onVideo: (stream: MediaStream) => void;
  onStreamInfo?: (info: {
    width: number;
    height: number;
    fps: number;
    codec: string;
  }) => void;
  onPadStats?: (hz: number, name: string) => void;
}

const SESSION_NOT_FOUND_RETRIES = 12;
const SESSION_NOT_FOUND_DELAY_MS = 750;

export class CouchlinkPlayer {
  private ws: WebSocket | null = null;
  private pc: RTCPeerConnection | null = null;
  private padDc: RTCDataChannel | null = null;
  private heartbeatTimer: number | null = null;
  private padTimer: number | null = null;
  private connectTimer: number | null = null;
  private sessionRetryTimer: number | null = null;
  private sessionRetries = 0;
  private pending: { sid: string; pin: string } | null = null;
  private seq = 0;
  private padSent = 0;
  private padWindowStart = 0;
  private padName = "none";
  private turn: { url: string; user: string; pass: string } | null = null;

  constructor(private cb: PlayerCallbacks) {}

  setTurn(turn: { url: string; user: string; pass: string } | null) {
    this.turn = turn;
  }

  connect(signalingUrl: string, sessionId: string, pin: string) {
    this.cleanup();
    this.sessionRetries = 0;
    const url = signalingUrl.trim();
    const sid = sessionId.trim().replace(/\s+/g, "");
    const pinCode = pin.trim().replace(/\D/g, "").slice(0, 6);
    this.pending = { sid, pin: pinCode };

    if (
      typeof window !== "undefined" &&
      window.location.protocol === "https:" &&
      url.startsWith("ws://")
    ) {
      this.cb.onState("error", "Use wss:// — this page is HTTPS");
      return;
    }
    if (!sid || pinCode.length < 4) {
      this.cb.onState("error", "Session ID and PIN required");
      return;
    }

    this.cb.onState("connecting", `Opening ${url}`);
    const ws = new WebSocket(url);
    this.ws = ws;

    this.connectTimer = window.setTimeout(() => {
      if (ws.readyState === WebSocket.CONNECTING) {
        ws.close();
        this.cb.onState(
          "error",
          "Timed out reaching signaling. Check WireGuard / LAN and that couchlink-signaling is running."
        );
      }
    }, 12_000);

    ws.onopen = () => {
      if (this.connectTimer) clearTimeout(this.connectTimer);
      this.connectTimer = null;
      this.cb.onState("registering");
      this.sendRegister(ws);
      this.startHeartbeat(ws);
    };

    ws.onmessage = async (ev) => {
      try {
        const msg = JSON.parse(ev.data as string) as SignalMessage;
        await this.handleSignal(msg);
      } catch (e) {
        this.cb.onState("error", `Bad message: ${e}`);
      }
    };

    ws.onerror = () => {
      if (this.connectTimer) clearTimeout(this.connectTimer);
      this.cb.onState("error", "WebSocket failed — wrong URL or cert not trusted");
    };

    ws.onclose = (ev) => {
      if (this.connectTimer) clearTimeout(this.connectTimer);
      if (ev.code !== 1000 && this.ws === ws) {
        this.cb.onState("error", ev.reason || `Closed (${ev.code})`);
      } else if (this.ws === ws) {
        this.cb.onState("disconnected");
      }
    };
  }

  disconnect() {
    this.cleanup();
    this.cb.onState("disconnected");
  }

  private sendRegister(ws: WebSocket) {
    const p = this.pending;
    if (!p || ws.readyState !== WebSocket.OPEN) return;
    send(ws, {
      type: "register_player",
      session_id: p.sid,
      pin: p.pin,
    });
  }

  private cleanup() {
    if (this.connectTimer) clearTimeout(this.connectTimer);
    if (this.sessionRetryTimer) clearTimeout(this.sessionRetryTimer);
    if (this.heartbeatTimer) clearInterval(this.heartbeatTimer);
    if (this.padTimer) cancelAnimationFrame(this.padTimer);
    this.connectTimer = null;
    this.sessionRetryTimer = null;
    this.heartbeatTimer = null;
    this.padTimer = null;
    this.padDc?.close();
    this.pc?.close();
    this.ws?.close();
    this.padDc = null;
    this.pc = null;
    this.ws = null;
  }

  private startHeartbeat(ws: WebSocket) {
    this.heartbeatTimer = window.setInterval(() => {
      if (ws.readyState === WebSocket.OPEN) send(ws, { type: "heartbeat" });
    }, 15000);
  }

  private async ensurePeer(): Promise<RTCPeerConnection> {
    if (this.pc) return this.pc;
    // Public STUN for NAT discovery, plus the host's own TURN relay (if given via
    // the invite link) for symmetric-NAT/CGNAT peers STUN alone can't punch through.
    const iceServers: RTCIceServer[] = [
      { urls: "stun:stun.l.google.com:19302" },
      { urls: "stun:stun1.l.google.com:19302" },
    ];
    if (this.turn) {
      iceServers.push({
        urls: this.turn.url,
        username: this.turn.user,
        credential: this.turn.pass,
      });
    }
    const pc = new RTCPeerConnection({ iceServers });
    this.pc = pc;

    pc.ontrack = (ev) => {
      const stream =
        ev.streams[0] ?? new MediaStream(ev.track ? [ev.track] : []);
      this.cb.onState("connected");
      this.cb.onVideo(stream);
    };

    pc.onicecandidate = (ev) => {
      if (ev.candidate && this.ws?.readyState === WebSocket.OPEN) {
        send(this.ws, {
          type: "ice_candidate",
          candidate: ev.candidate.candidate,
          sdpMid: ev.candidate.sdpMid,
          sdpMLineIndex: ev.candidate.sdpMLineIndex ?? undefined,
        });
      }
    };

    pc.ondatachannel = (ev) => {
      const ch = ev.channel;
      if (ch.label === PAD_CHANNEL) {
        this.padDc = ch;
        ch.binaryType = "arraybuffer";
        ch.onopen = () => {
          this.cb.onState("connected", "pad open");
          this.startPadLoop();
        };
      }
    };

    return pc;
  }

  private startPadLoop() {
    this.padWindowStart = performance.now();
    this.padSent = 0;
    const tick = () => {
      this.padTimer = requestAnimationFrame(tick);
      this.pollAndSendPad();
    };
    this.padTimer = requestAnimationFrame(tick);
  }

  private pollAndSendPad() {
    if (this.padDc?.readyState !== "open") return;
    const pads = navigator.getGamepads?.() ?? [];
    let gp: Gamepad | null = null;
    for (const p of pads) {
      if (p) {
        gp = p;
        break;
      }
    }
    if (!gp) return;
    this.padName = gp.id;
    this.seq = (this.seq + 1) >>> 0;
    const state: PadState = fromBrowserGamepad(gp, this.seq);
    this.padDc.send(encodeClpd(state));
    this.padSent += 1;
    const now = performance.now();
    if (now - this.padWindowStart >= 1000) {
      this.cb.onPadStats?.(this.padSent, this.padName);
      this.padSent = 0;
      this.padWindowStart = now;
    }
  }

  private async handleSignal(msg: SignalMessage) {
    switch (msg.type) {
      case "registered":
        this.sessionRetries = 0;
        this.cb.onState("waiting_host", "Waiting for host offer…");
        break;
      case "error": {
        const text = msg.message || "";
        const notReady = /unknown session|session not found|not connected/i.test(
          text
        );
        if (
          notReady &&
          this.ws?.readyState === WebSocket.OPEN &&
          this.sessionRetries < SESSION_NOT_FOUND_RETRIES
        ) {
          this.sessionRetries += 1;
          this.cb.onState(
            "registering",
            `Waiting for host… (${this.sessionRetries}/${SESSION_NOT_FOUND_RETRIES})`
          );
          if (this.sessionRetryTimer) clearTimeout(this.sessionRetryTimer);
          this.sessionRetryTimer = window.setTimeout(() => {
            if (this.ws) this.sendRegister(this.ws);
          }, SESSION_NOT_FOUND_DELAY_MS);
          break;
        }
        this.cb.onState("error", text);
        break;
      }
      case "offer": {
        this.cb.onState("negotiating");
        const pc = await this.ensurePeer();
        await pc.setRemoteDescription({ type: "offer", sdp: msg.sdp });
        const answer = await pc.createAnswer();
        await pc.setLocalDescription(answer);
        if (this.ws) send(this.ws, { type: "answer", sdp: answer.sdp! });
        break;
      }
      case "ice_candidate":
        if (this.pc) {
          await this.pc.addIceCandidate({
            candidate: msg.candidate,
            sdpMid: msg.sdpMid ?? undefined,
            sdpMLineIndex: msg.sdpMLineIndex ?? undefined,
          });
        }
        break;
      case "stream_info":
        this.cb.onStreamInfo?.(msg);
        break;
      case "peer_left":
        this.cb.onState("waiting_host", "Host disconnected");
        break;
    }
  }
}
