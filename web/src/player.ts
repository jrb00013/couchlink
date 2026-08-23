import { encodeClpd, fromBrowserGamepad, PAD_CHANNEL, type PadState } from "./clpd";
import { KeyboardMouseInput } from "./keyboardMouse";
import { controllerKind, selectPhysicalGamepads } from "./controllerKind";
import { TouchGamepadInput } from "./touchPad";
import {
  ClvdAssembler,
  decodeClvdFragment,
  PLI_BYTES,
  VIDEO_CHANNEL,
  type VideoAccessUnit,
} from "./clvd";
import { clog, cerror, cwarn } from "./log";
import { jitterWindow } from "./latencyStats";
import { send, type SignalMessage } from "./proto";
import { canUseWebCodecs } from "./webCodecsCanvas";
import { echoAgeOnce, type AgeEcho } from "./ageEcho";
import { notePadSent } from "./inputPhoton";

export type ConnectionState =
  | "disconnected"
  | "connecting"
  | "registering"
  | "waiting_host"
  | "negotiating"
  | "connected"
  | "error";

export type PresentPath = "webcodecs" | "rtp" | "warmup";

export interface PlayerCallbacks {
  onState: (s: ConnectionState, detail?: string) => void;
  onVideo: (stream: MediaStream) => void;
  /** Annex-B access units from the unordered `video` DataChannel.
   * `recvMs` is performance.now() at fragment assemble — use for age budget. */
  onVideoAccessUnit?: (au: VideoAccessUnit, recvMs: number) => void;
  /** Fired when the preferred present path is known. */
  onPresentPath?: (path: PresentPath, detail?: string) => void;
  onStreamInfo?: (info: {
    width: number;
    height: number;
    fps: number;
    codec: string;
    capture_ok?: boolean;
    capture_hint?: string;
  }) => void;
  /** Host pipeline stage timings + commanded encoder target, ~5s tick. */
  onHostStats?: (stats: {
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
    age_p50_ms?: number;
    age_p95_ms?: number;
  }) => void;
  onPadStats?: (hz: number, name: string) => void;
  /** This browser's own assigned slot (1-based), so it can label itself
   * correctly in a per-player display instead of guessing. */
  onRegistered?: (slot: number) => void;
  /** Session occupancy ("N/3 players connected") broadcast on join/leave. */
  onPlayersStatus?: (occupied: number, max: number) => void;
  /** Every seated player's controller family — broadcast so a controller
   * debug view can show everyone's pad, not just this browser's own. */
  onPlayerPadInfo?: (slot: number, kind: string, id: string) => void;
  /** A fellow player (not the host) left — drop their entry from any
   * per-player display instead of leaving a stale "connected" row. */
  onPlayerLeft?: (slot: number) => void;
  /** Full getStats-derived telemetry snapshot, ~2s tick. */
  onTelemetry?: (t: PlayerTelemetry) => void;
}

export type MediaPathStats = {
  local: string;
  remote: string;
  family: "IPv4" | "IPv6";
  protocol: string;
  relayed: boolean;
  rttMs: number;
};

export type InboundVideoStats = {
  jitterBufferMs: number;
  decodeFps: number;
  framesDropped: number;
  framesDecoded: number;
  bitrateKbps: number;
  bytesReceived: number;
  packetsLost: number;
  packetsReceived: number;
  packetLossPct: number;
  jitterMs: number;
  frameWidth: number;
  frameHeight: number;
  framesPerSecond: number;
  pauseCount: number;
  freezeCount: number;
  totalFreezesDuration: number;
};

export type PlayerTelemetry = {
  path: MediaPathStats | null;
  video: InboundVideoStats | null;
  padHz: number;
  padName: string;
  at: number;
};

const SESSION_NOT_FOUND_RETRIES = 12;
const SESSION_NOT_FOUND_DELAY_MS = 750;
/** How long to wait before triggering a peer reset when media was previously healthy.
 *  TURN paths regularly bounce ICE failed→connected; give them time to self-heal. */
const MEDIA_RECOVER_DELAY_MS = 12_000;
/** Shorter delay when the peer was never healthy (first-connect failure). */
const MEDIA_RECOVER_DELAY_COLD_MS = 5_000;
/** 250Hz — matches the native client and keeps sampling off the display clock. */
const PAD_POLL_MS = 4;

function preferLegacyRtp(): boolean {
  if (typeof location === "undefined") return false;
  return new URLSearchParams(location.search).get("legacyVideo") === "1";
}

export class CouchlinkPlayer {
  private ws: WebSocket | null = null;
  private pc: RTCPeerConnection | null = null;
  private padDc: RTCDataChannel | null = null;
  private videoDc: RTCDataChannel | null = null;
  /** True when CLVD + WebCodecs is the active present path (skip RTP attach). */
  private webcodecsPath = false;
  private clvdAsm = new ClvdAssembler();
  private heartbeatTimer: number | null = null;
  private statsTimer: number | null = null;
  private lastStats: { delay: number; count: number; decoded: number } | null =
    null;
  private padTimer: number | null = null;
  private connectTimer: number | null = null;
  private sessionRetryTimer: number | null = null;
  private mediaRecoverTimer: number | null = null;
  private iceDisconnectTimer: number | null = null;
  private sessionRetries = 0;
  private pending: { sid: string; pin: string } | null = null;
  private seq = 0;
  /** Soft-hold previous pad for one missed digital poll. */
  private lastPadHold: PadState | null = null;
  private lastPadHoldAt = 0;
  private padSent = 0;
  private padWindowStart = 0;
  private padName = "none";
  /** Last 1s pad send-rate reported to the UI, reused in telemetry ticks. */
  private lastPadHz = 0;
  /** Last Gamepad.id announced to the host, so pad_info is sent on change… */
  private padInfoSent = "";
  /** …and re-sent on this cadence regardless, so `player_pad_info` doubles as
   * a "still linked and actually sending input" heartbeat other players can
   * see — a one-time report on connect can't tell anyone the controller is
   * still alive an hour later. */
  private padInfoLastSentAt = 0;
  private static readonly PAD_INFO_HEARTBEAT_MS = 3000;
  /** Last logged media-path summary, so the line prints only on change. */
  private lastPathKey = "";
  /** Keyboard+mouse input source — injected by the UI, null if not active. */
  private kbm: KeyboardMouseInput | null = null;
  /** Touch-screen controller — injected by the UI on mobile, null otherwise. */
  private touch: TouchGamepadInput | null = null;
  /** Previous inbound-rtp sample, for bitrate + loss deltas. */
  private lastInbound:
    | { bytes: number; lost: number; count: number; at: number }
    | null = null;
  /** Last present path reported to the host, so it is sent only on change. */
  private presentPathSent: PresentPath | "" = "";
  private turn: { url: string; user: string; pass: string } | null = null;
  private gotVideoTrack = false;
  private lastOfferEpoch = 0;
  private mediaHealthy = false;
  /** Re-asserted each stats tick — Chrome grows the JB under jitter. */
  private videoReceiver: (RTCRtpReceiver & {
    jitterBufferTarget?: number | null;
    playoutDelayHint?: number | null;
  }) | null = null;

  constructor(private cb: PlayerCallbacks) {}

  setTurn(turn: { url: string; user: string; pass: string } | null) {
    this.turn = turn;
  }

  /** Attach or detach a keyboard/mouse input source. Call with null to disable. */
  setKbm(kbm: KeyboardMouseInput | null) {
    this.kbm = kbm;
  }

  /** Attach or detach the mobile touch controller. Call with null to disable. */
  setTouchInput(touch: TouchGamepadInput | null) {
    this.touch = touch;
  }

  connect(signalingUrl: string, sessionId: string, pin: string) {
    clog("connect()", { signalingUrl, sessionId, pinLen: pin.length });
    this.lastOfferEpoch = 0;
    this.mediaHealthy = false;
    this.webcodecsPath = false;
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
      clog("websocket open");
      if (this.connectTimer) clearTimeout(this.connectTimer);
      this.connectTimer = null;
      this.cb.onState("registering");
      this.sendRegister(ws);
      this.startHeartbeat(ws);
    };

    ws.onmessage = async (ev) => {
      try {
        const msg = JSON.parse(ev.data as string) as SignalMessage;
        clog("signal ←", msg.type, msg.type === "offer" ? `(sdp ${msg.sdp?.length ?? 0} chars)` : "");
        await this.handleSignal(msg);
      } catch (e) {
        cerror("bad signal message", e, ev.data);
        this.cb.onState("error", `Bad message: ${e}`);
      }
    };

    ws.onerror = (ev) => {
      cerror("websocket error", ev);
      if (this.connectTimer) clearTimeout(this.connectTimer);
      this.cb.onState("error", "WebSocket failed — wrong URL or cert not trusted");
    };

    ws.onclose = (ev) => {
      clog("websocket close", { code: ev.code, reason: ev.reason, wasClean: ev.wasClean });
      if (this.connectTimer) clearTimeout(this.connectTimer);
      if (ev.code !== 1000 && this.ws === ws) {
        this.cb.onState("error", ev.reason || `Closed (${ev.code})`);
      } else if (this.ws === ws) {
        this.cb.onState("disconnected");
      }
    };
  }

  disconnect() {
    clog("disconnect()");
    this.cleanup();
    this.cb.onState("disconnected");
  }

  /**
   * Report the present path to the UI, and to the host over signaling.
   *
   * Before this the host wrote every frame to RTP *and* the DataChannel,
   * because it had no way to know which one the browser paints — double the
   * per-frame send work, and two streams competing inside one congestion
   * controller. Sent only on change, and only once the socket is open; a path
   * decided before `register_player` completes is reported as soon as it can be.
   */
  private notifyPresentPath(path: PresentPath, detail?: string) {
    this.cb.onPresentPath?.(path, detail);
    if (this.presentPathSent === path) return;
    if (this.ws?.readyState !== WebSocket.OPEN) return;
    this.presentPathSent = path;
    send(this.ws, { type: "present_path", path });
    clog("signal → present_path", path);
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

  /**
   * Report which ICE candidate pair media is actually using, and its RTT.
   *
   * Nothing in the system logged this, so "the online path is slow" could not
   * be distinguished between a direct IPv6 route, an IPv4 hole-punch, and a
   * TURN relay — three paths with very different costs. `currentRoundTripTime`
   * on the succeeded pair is also the only honest transit measurement we have:
   * it is measured on the media path itself, not inferred from a ping to some
   * unrelated host.
   *
   * Logged only when the pair or the rounded RTT changes, so it does not spam
   * a line every poll.
   */
  private logSelectedPath(stats: RTCStatsReport): MediaPathStats | null {
    let pair: any = null;
    const byId = new Map<string, any>();
    stats.forEach((r: any) => byId.set(r.id, r));
    stats.forEach((r: any) => {
      if (r.type === "candidate-pair" && (r.selected || r.state === "succeeded")) {
        // Prefer the nominated pair when the browser marks one.
        if (!pair || r.nominated) pair = r;
      }
    });
    if (!pair) return null;
    const local = byId.get(pair.localCandidateId);
    const remote = byId.get(pair.remoteCandidateId);
    const rttMs = Math.round((pair.currentRoundTripTime ?? 0) * 1000);
    const relayed =
      local?.candidateType === "relay" || remote?.candidateType === "relay";
    const result: MediaPathStats = {
      local: local?.candidateType ?? "?",
      remote: remote?.candidateType ?? "?",
      family: local?.address?.includes(":") ? "IPv6" : "IPv4",
      protocol: local?.protocol ?? "?",
      relayed,
      rttMs,
    };
    const key = `${result.local}/${result.remote}/${rttMs}`;
    if (key === this.lastPathKey) return result;
    this.lastPathKey = key;
    clog("media path", result);
    return result;
  }

  private collectInbound(stats: RTCStatsReport): InboundVideoStats | null {
    let r: any = null;
    stats.forEach((s: any) => {
      if (s.type === "inbound-rtp" && s.kind === "video") r = s;
    });
    if (!r) return null;
    const now = performance.now();
    const prev = this.lastInbound;
    const bytes = r.bytesReceived ?? 0;
    const lost = r.packetsLost ?? 0;
    const count = r.jitterBufferEmittedCount ?? 0;
    const decoded = r.framesDecoded ?? 0;
    const bitrateKbps = prev
      ? Math.max(0, Math.round(((bytes - prev.bytes) * 8) / Math.max(1, now - prev.at)))
      : 0;
    const lostDelta = prev ? Math.max(0, lost - prev.lost) : 0;
    const received = r.packetsReceived ?? 0;
    const packetLossPct =
      lostDelta + received > 0 ? (lostDelta / (lostDelta + received)) * 100 : 0;
    this.lastInbound = { bytes, lost, count, at: now };

    // jitterBufferMs / decodeFps need the delta over the polling window, so we
    // can't derive them from the cumulative inbound-rtp row alone.
    const prevStats = this.lastStats;
    this.lastStats = { delay: r.jitterBufferDelay ?? 0, count, decoded };
    let jitterBufferMs = 0;
    let decodeFps = 0;
    if (prev && prevStats && count > prevStats.count) {
      const w = jitterWindow(
        {
          jitterBufferDelay: prevStats.delay,
          jitterBufferEmittedCount: prevStats.count,
          framesDecoded: prevStats.decoded,
          framesDropped: 0,
        },
        {
          jitterBufferDelay: r.jitterBufferDelay ?? 0,
          jitterBufferEmittedCount: count,
          framesDecoded: decoded,
          framesDropped: r.framesDropped ?? 0,
        },
        (now - prev.at) / 1000
      );
      if (w) {
        jitterBufferMs = w.jitterBufferMs;
        decodeFps = w.decodeFps;
      }
    }
    // Chrome will grow the JB after packet jitter; pin it back every poll.
    this.pinJitterBuffer();
    return {
      jitterBufferMs,
      decodeFps,
      framesDropped: r.framesDropped ?? 0,
      framesDecoded: decoded,
      bitrateKbps,
      bytesReceived: bytes,
      packetsLost: r.packetsLost ?? 0,
      packetsReceived: received,
      packetLossPct,
      jitterMs: (r.jitter ?? 0) * 1000,
      frameWidth: r.frameWidth ?? 0,
      frameHeight: r.frameHeight ?? 0,
      framesPerSecond: r.framesPerSecond ?? 0,
      pauseCount: r.pauseCount ?? 0,
      freezeCount: r.freezeCount ?? 0,
      totalFreezesDuration: r.totalFreezesDuration ?? 0,
    };
  }

  /**
   * The browser is the one segment the host cannot measure. jitterBufferDelay
   * divided by jitterBufferEmittedCount is the average time each frame sat in
   * Chrome's buffer before being shown — that is felt latency the host's stage
   * timings are completely blind to.
   */
  private startStatsPolling() {
    if (this.statsTimer) return;
    this.statsTimer = window.setInterval(async () => {
      const pc = this.pc;
      if (!pc) return;
      try {
        const stats = await pc.getStats();
        const path = this.logSelectedPath(stats);
        const video = this.collectInbound(stats);
        this.cb.onTelemetry?.({
          path,
          video,
          padHz: this.lastPadHz,
          padName: this.padName,
          at: performance.now(),
        });
        if (video && video.framesDecoded > 0) {
          clog("video stats", {
            jitterBufferMs: Math.round(video.jitterBufferMs),
            decodeFps: Math.round(video.decodeFps),
            framesDropped: video.framesDropped,
            frameHeight: video.frameHeight,
            pauseCount: video.pauseCount,
            freezeCount: video.freezeCount,
            totalFreezesDuration: video.totalFreezesDuration,
            jbTarget: this.videoReceiver?.jitterBufferTarget ?? null,
          });
        }
      } catch (e) {
        cwarn("getStats failed", String(e));
      }
    }, 2000);
  }

  private pinJitterBuffer() {
    const receiver = this.videoReceiver;
    if (!receiver) return;
    try {
      if ("jitterBufferTarget" in receiver) receiver.jitterBufferTarget = 0;
      if ("playoutDelayHint" in receiver) receiver.playoutDelayHint = 0;
    } catch {
      /* older Chromium */
    }
  }

  private cleanup() {
    clog("cleanup()", {
      hadWs: !!this.ws,
      hadPc: !!this.pc,
      pcState: this.pc?.connectionState,
      iceState: this.pc?.iceConnectionState,
    });
    if (this.connectTimer) clearTimeout(this.connectTimer);
    if (this.sessionRetryTimer) clearTimeout(this.sessionRetryTimer);
    if (this.mediaRecoverTimer) clearTimeout(this.mediaRecoverTimer);
    if (this.iceDisconnectTimer) clearTimeout(this.iceDisconnectTimer);
    if (this.heartbeatTimer) clearInterval(this.heartbeatTimer);
    if (this.statsTimer) clearInterval(this.statsTimer);
    if (this.padTimer) clearInterval(this.padTimer);
    this.connectTimer = null;
    this.sessionRetryTimer = null;
    this.mediaRecoverTimer = null;
    this.iceDisconnectTimer = null;
    this.heartbeatTimer = null;
    this.statsTimer = null;
    this.lastStats = null;
    this.lastInbound = null;
    this.padTimer = null;
    this.resetPeer();
    this.ws?.close();
    this.ws = null;
  }

  /** Drop WebRTC only; keep signaling socket if open. */
  private resetPeer() {
    if (this.padTimer) clearInterval(this.padTimer);
    this.padTimer = null;
    if (this.iceDisconnectTimer) clearTimeout(this.iceDisconnectTimer);
    this.iceDisconnectTimer = null;
    this.padDc?.close();
    this.videoDc?.close();
    this.pc?.close();
    this.padDc = null;
    this.videoDc = null;
    this.webcodecsPath = false;
    this.pc = null;
    this.videoReceiver = null;
    this.gotVideoTrack = false;
    this.mediaHealthy = false;
  }

  private scheduleMediaRecover(reason: string) {
    if (this.mediaRecoverTimer || !this.ws || this.ws.readyState !== WebSocket.OPEN) {
      return;
    }
    if (this.mediaHealthy) {
      clog("ICE blip while video healthy — waiting before recover", reason);
    }
    cwarn("scheduling media recover", reason);
    this.cb.onState("waiting_host", `Media lost (${reason}) — reconnecting…`);
    this.mediaRecoverTimer = window.setTimeout(() => {
      this.mediaRecoverTimer = null;
      this.mediaHealthy = false;
      this.resetPeer();
      if (this.ws?.readyState === WebSocket.OPEN) {
        clog("signal → request_offer (media recover)");
        send(this.ws, { type: "request_offer" });
        this.cb.onState("waiting_host", "Recovering media…");
      }
    }, this.mediaHealthy ? MEDIA_RECOVER_DELAY_MS : MEDIA_RECOVER_DELAY_COLD_MS);
  }

  private async applyRemoteOffer(sdp: string, epoch: number) {
    const pc0 = this.pc;
    const healthy =
      !!pc0 &&
      this.gotVideoTrack &&
      this.mediaHealthy &&
      pc0.connectionState === "connected" &&
      pc0.signalingState === "stable";

    if (healthy && epoch > 0 && epoch <= this.lastOfferEpoch) {
      clog("ignore stale offer", { epoch, last: this.lastOfferEpoch });
      return;
    }

    // Host rebuilt peer (new player WS) or cold join → new RTCPeerConnection.
    const hostRebuilt = epoch > this.lastOfferEpoch + 1;
    const needNewPc =
      !pc0 ||
      hostRebuilt ||
      pc0.connectionState === "failed" ||
      pc0.connectionState === "closed";

    if (pc0 && needNewPc) {
      clog("offer — new RTCPeerConnection", { epoch, hostRebuilt, state: pc0.connectionState });
      this.resetPeer();
    } else if (pc0 && healthy) {
      clog("offer — renegotiate in place", { epoch });
    }

    this.cb.onState("negotiating");
    const pc = await this.ensurePeer();
    await pc.setRemoteDescription({ type: "offer", sdp });
    clog("setRemoteDescription(offer) ok", pc.signalingState);
    const answer = await pc.createAnswer();
    await pc.setLocalDescription(answer);
    clog("setLocalDescription(answer) ok", pc.signalingState);
    if (this.ws) send(this.ws, { type: "answer", sdp: answer.sdp!, epoch });
    clog("signal → answer", `(sdp ${answer.sdp?.length ?? 0} chars, epoch ${epoch})`);
    if (epoch > 0) {
      this.lastOfferEpoch = epoch;
    } else {
      this.lastOfferEpoch += 1;
    }
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
      // UDP + TCP TURN — WSL / carrier NATs often need TCP when UDP fails.
      const urls = [this.turn.url];
      if (!/transport=tcp/i.test(this.turn.url)) {
        urls.push(
          this.turn.url.includes("?")
            ? `${this.turn.url}&transport=tcp`
            : `${this.turn.url}?transport=tcp`,
        );
      }
      iceServers.push({
        urls,
        username: this.turn.user,
        credential: this.turn.pass,
      });
    }
    const pc = new RTCPeerConnection({ iceServers });
    this.pc = pc;
    clog("RTCPeerConnection created", { iceServers: iceServers.map((s) => s.urls) });

    pc.onconnectionstatechange = () => {
      clog("pc.connectionState", pc.connectionState);
      if (pc.connectionState === "connected") {
        // Authoritative healthy signal — both ICE and DTLS are up.
        this.mediaHealthy = this.gotVideoTrack;
        if (this.mediaRecoverTimer) {
          clearTimeout(this.mediaRecoverTimer);
          this.mediaRecoverTimer = null;
        }
      } else if (pc.connectionState === "disconnected") {
        // Transient loss — schedule a recover with the full grace period.
        // If ICE self-heals the timer will be cancelled before it fires.
        this.scheduleMediaRecover("connection disconnected");
      } else if (pc.connectionState === "failed") {
        cwarn("WebRTC connection failed — scheduling recover");
        this.scheduleMediaRecover("connection failed");
      }
    };
    pc.oniceconnectionstatechange = () => {
      clog("pc.iceConnectionState", pc.iceConnectionState);
      if (pc.iceConnectionState === "connected" || pc.iceConnectionState === "completed") {
        // ICE layer is up — mark healthy and cancel any pending recover timer.
        // connectionState may lag behind iceConnectionState on some browsers.
        this.mediaHealthy = this.gotVideoTrack;
        if (this.mediaRecoverTimer) {
          clog("ICE reconnected — cancelling media recover timer");
          clearTimeout(this.mediaRecoverTimer);
          this.mediaRecoverTimer = null;
        }
      } else if (pc.iceConnectionState === "disconnected") {
        cwarn("ICE disconnected (may recover on its own)", pc.iceConnectionState);
        // "disconnected" = browser's consent-freshness pings stopped being answered.
        // A NAT rebind or brief drop on a live direct P2P path (srflx/srflx) does
        // exactly this. restartIce() is far cheaper than waiting for `failed` to
        // trigger a full signaling round-trip — try it after a 4s grace period.
        if (!this.iceDisconnectTimer) {
          this.iceDisconnectTimer = window.setTimeout(() => {
            this.iceDisconnectTimer = null;
            if (this.pc?.iceConnectionState !== "disconnected") return;
            clog("ICE still disconnected after grace — restarting ICE");
            try {
              this.pc.restartIce();
            } catch (e) {
              cwarn("restartIce failed", String(e));
            }
          }, 4000);
        }
      } else if (pc.iceConnectionState === "failed") {
        if (this.iceDisconnectTimer) {
          clearTimeout(this.iceDisconnectTimer);
          this.iceDisconnectTimer = null;
        }
        cwarn("ICE failed — scheduling recover", pc.iceConnectionState);
        this.scheduleMediaRecover("ICE failed");
      } else {
        // connected / completed / closed — cancel any pending ICE restart timer
        if (this.iceDisconnectTimer) {
          clearTimeout(this.iceDisconnectTimer);
          this.iceDisconnectTimer = null;
        }
      }
    };
    pc.onicegatheringstatechange = () => {
      clog("pc.iceGatheringState", pc.iceGatheringState);
    };
    pc.onsignalingstatechange = () => {
      clog("pc.signalingState", pc.signalingState);
    };

    pc.ontrack = (ev) => {
      const track = ev.track;
      // Chrome buffers received video before playing it — typically 100-200ms of
      // pure added input lag that no host-side measurement can see. For a co-play
      // stream, being a frame behind beats being smooth and late, so ask for the
      // shallowest buffer the browser will give us. Both properties are
      // Chromium-only and versioned, hence the guarded assignment.
      const receiver = ev.receiver as RTCRtpReceiver & {
        jitterBufferTarget?: number | null;
        playoutDelayHint?: number | null;
      };
      this.videoReceiver = receiver;
      this.pinJitterBuffer();
      clog("requested minimum jitter buffer", {
        jitterBufferTarget: receiver.jitterBufferTarget,
        playoutDelayHint: receiver.playoutDelayHint,
      });
      clog("ontrack", {
        kind: track.kind,
        id: track.id,
        readyState: track.readyState,
        muted: track.muted,
        streamIds: ev.streams.map((s) => s.id),
      });
      track.onmute = () => cwarn("track muted", track.kind, track.id);
      track.onunmute = () => {
        clog("track unmuted", track.kind, track.id);
      };
      track.onended = () => clog("track ended", track.kind, track.id);
      if (track.kind === "video" && "contentHint" in track) {
        track.contentHint = "detail";
      }
      const stream =
        ev.streams[0] ?? new MediaStream(ev.track ? [ev.track] : []);
      clog("attaching MediaStream", {
        id: stream.id,
        tracks: stream.getTracks().map((t) => `${t.kind}:${t.readyState}`),
      });
      this.gotVideoTrack = true;
      this.startStatsPolling();
      this.cb.onState("connected", "video track");
      // Always deliver the stream so the UI can fall back if WebCodecs never paints.
      if (this.webcodecsPath) {
        clog("RTP track received — painted as safety net until WebCodecs paints");
        this.cb.onVideo(stream);
        return;
      }
      this.notifyPresentPath("rtp");
      this.cb.onVideo(stream);
    };

    pc.onicecandidate = (ev) => {
      if (ev.candidate) {
        clog("local ICE", ev.candidate.candidate?.slice(0, 80));
        if (this.ws?.readyState === WebSocket.OPEN) {
          send(this.ws, {
            type: "ice_candidate",
            candidate: ev.candidate.candidate,
            sdpMid: ev.candidate.sdpMid,
            sdpMLineIndex: ev.candidate.sdpMLineIndex ?? undefined,
          });
        }
      } else {
        clog("local ICE gathering complete (null candidate)");
      }
    };

    pc.ondatachannel = (ev) => {
      const ch = ev.channel;
      clog("datachannel", ch.label, ch.readyState);
      if (ch.label === PAD_CHANNEL) {
        this.padDc = ch;
        ch.binaryType = "arraybuffer";
        ch.onopen = () => {
          clog("pad datachannel open");
          if (!this.gotVideoTrack && !this.webcodecsPath) {
            this.cb.onState("connected", "pad open (no video track yet)");
          }
          this.startPadLoop();
        };
        ch.onclose = () => clog("pad datachannel closed");
        ch.onerror = (e) => cerror("pad datachannel error", e);
      } else if (ch.label === VIDEO_CHANNEL) {
        this.bindVideoChannel(ch);
      }
    };

    return pc;
  }

  private bindVideoChannel(ch: RTCDataChannel) {
    this.videoDc = ch;
    ch.binaryType = "arraybuffer";
    const useWc = canUseWebCodecs() && !preferLegacyRtp();
    ch.onopen = () => {
      clog("video datachannel open", {
        secureContext: window.isSecureContext,
        webcodecs: useWc,
      });
      if (useWc) {
        this.webcodecsPath = true;
        // Warm-up: tell the host to keep BOTH paths live. Announcing
        // "webcodecs" now would make it cut RTP while this fresh DataChannel
        // is still in SCTP slow-start — RTP stops, the keyframe stalls, the
        // decoder never configures, and the 2.5s fallback lands the viewer on
        // RTP-with-jitter-buffer for the rest of the session. Stay "warmup"
        // (both paths) until the first frame actually paints, then promote.
        this.notifyPresentPath(
          "warmup",
          "CLVD DataChannel + WebCodecs warming — RTP stays live as safety net"
        );
        this.cb.onState("connected", "webcodecs video");
        this.gotVideoTrack = true;
        this.mediaHealthy = true;
        // Ask for IDR immediately — we may have joined mid-GOP.
        this.requestVideoKeyframe();
      } else {
        cwarn(
          "video DataChannel open but WebCodecs unavailable — using RTP path",
          {
            secureContext: window.isSecureContext,
            hasDecoder: typeof VideoDecoder === "function",
          }
        );
        this.notifyPresentPath(
          "rtp",
          window.isSecureContext
            ? "WebCodecs missing"
            : "insecure context (use http://127.0.0.1 or https)"
        );
      }
    };
    ch.onmessage = (ev) => {
      if (!this.webcodecsPath) return;
      const data = ev.data;
      if (!(data instanceof ArrayBuffer) && !ArrayBuffer.isView(data)) return;
      const frag = decodeClvdFragment(data as ArrayBuffer);
      if (!frag) return;
      const au = this.clvdAsm.push(frag);
      if (!au) return;
      // Age is measured at paint, not receive — see echoPaintedAge().
      this.cb.onVideoAccessUnit?.(au, performance.now());
    };
    ch.onclose = () => clog("video datachannel closed");
    ch.onerror = (e) => cerror("video datachannel error", e);
  }

  /**
   * Echo receive→paint age on the pad DataChannel once per AU seq.
   * Call from the presentation path after the frame is actually drawn —
   * not when the access unit arrives (that under-reported age as ~0).
   */
  echoPaintedAge(e: AgeEcho) {
    const pad = this.padDc;
    if (!pad || pad.readyState !== "open") return;
    echoAgeOnce(e, (json) => {
      try {
        pad.send(json);
      } catch {
        /* pad closing */
      }
    });
  }

  /** Tell the host we need an IDR (any message on the video DC). */
  requestVideoKeyframe() {
    const dc = this.videoDc;
    if (!dc || dc.readyState !== "open") return;
    try {
      dc.send(PLI_BYTES);
    } catch (e) {
      cwarn("pli send failed", String(e));
    }
  }

  /** Stop preferring CLVD/WebCodecs — UI is switching to the RTP present path. */
  preferRtpPresent() {
    this.webcodecsPath = false;
    this.notifyPresentPath("rtp", "WebCodecs fallback");
  }

  /** WebCodecs went dark — keep both paths; the UI is switching to the live RTP canvas. */
  resumeWarmup() {
    this.notifyPresentPath(
      "warmup",
      "WebCodecs stalled — RTP stays live"
    );
  }

  /**
   * WebCodecs painted its first frame. Promote to "webcodecs" so the host
   * thins RTP to IDR-only (path_flags still keeps the track alive). Staying
   * on "warmup" forever forced full dual-send and blew the push budget.
   */
  promoteWebcodecs() {
    if (!this.webcodecsPath) return;
    this.notifyPresentPath(
      "webcodecs",
      "CLVD DataChannel + WebCodecs present — RTP off (stall → warmup rescue)"
    );
  }

  /**
   * Poll the pad on a timer, not requestAnimationFrame.
   *
   * rAF fires at the display refresh — 60Hz on most screens — so every input
   * carried up to 16.7ms of quantisation before it even left the browser, while
   * the native client polls at 250Hz. The Gamepad API has no rAF requirement;
   * getGamepads() reads current state whenever it is called.
   *
   * rAF also stops entirely when the tab is backgrounded, which silently killed
   * input the moment the player alt-tabbed.
   */
  private startPadLoop() {
    this.padWindowStart = performance.now();
    this.padSent = 0;
    this.lastPadHold = null;
    this.lastPadHoldAt = 0;
    this.padTimer = window.setInterval(() => this.pollAndSendPad(), PAD_POLL_MS);
  }

  /** Keep digital buttons one poll if Gamepad API glitched empty. */
  private holdDigitalOneTick(state: PadState, now: number): PadState {
    const TICK_MS = 5;
    const prev = this.lastPadHold;
    if (!prev || now - this.lastPadHoldAt > TICK_MS) return state;
    if (state.buttons === 0 && prev.buttons !== 0) {
      return { ...state, buttons: prev.buttons };
    }
    return state;
  }

  private emitPad(state: PadState) {
    if (this.padDc?.readyState !== "open") return;
    const now = performance.now();
    const held = this.holdDigitalOneTick(state, now);
    this.lastPadHold = held;
    this.lastPadHoldAt = now;
    this.padDc.send(encodeClpd({ ...held, clientTsMs: now >>> 0 }));
    notePadSent(now, held.seq);
    this.padSent += 1;
  }

  private pollAndSendPad() {
    if (this.padDc?.readyState !== "open") return;
    const pads = navigator.getGamepads?.() ?? [];
    const physical = selectPhysicalGamepads(
      [...pads].filter((p): p is Gamepad => !!p)
    );
    const gp = physical[0] ?? null;
    if (!gp) {
      // No gamepad — fall back to the touch controller (mobile), else
      // keyboard/mouse.
      const touch = this.touch;
      if (touch) {
        this.seq = (this.seq + 1) >>> 0;
        const state = touch.sample(this.seq);
        this.emitPad(state);
        // "generic" selects the emulator-side virtual pad *backend*
        // (backend_for()'s catch-all, XInput), not the wire format — CLPD
        // frames are the same shape regardless of source. Touch has no real
        // controller identity to report, so "generic" is the honest kind —
        // and it matters which backend that maps to: the ViGEm DS4 backend
        // (what "dualsense" used to route to) is DirectInput-shaped, and
        // plenty of co-op games — Marvel Ultimate Alliance 3 among them —
        // simply never see it as a joinable local player. XInput does.
        if (
          this.padInfoSent !== "touch" ||
          performance.now() - this.padInfoLastSentAt > CouchlinkPlayer.PAD_INFO_HEARTBEAT_MS
        ) {
          this.padInfoSent = "touch";
          this.padInfoLastSentAt = performance.now();
          if (this.ws?.readyState === WebSocket.OPEN) {
            send(this.ws, { type: "pad_info", kind: "generic", id: "touch" });
          }
        }
        this.padName = "touch";
        const now = performance.now();
        if (now - this.padWindowStart >= 1000) {
          this.lastPadHz = this.padSent;
          this.cb.onPadStats?.(this.padSent, "touch");
          this.padSent = 0;
          this.padWindowStart = now;
        }
        return;
      }
      // No gamepad — fall back to keyboard/mouse if active
      const kbm = this.kbm;
      if (!kbm) return;
      this.seq = (this.seq + 1) >>> 0;
      const kbmState = kbm.sample(this.seq);
      this.emitPad(kbmState);
      // See the touch branch above for why this is "generic" and not
      // "dualsense": no real controller identity to report, and the
      // backend that maps to (XInput) is what actually gets recognized as
      // a joinable player in most co-op games.
      if (
        this.padInfoSent !== "keyboard" ||
        performance.now() - this.padInfoLastSentAt > CouchlinkPlayer.PAD_INFO_HEARTBEAT_MS
      ) {
        this.padInfoSent = "keyboard";
        this.padInfoLastSentAt = performance.now();
        if (this.ws?.readyState === WebSocket.OPEN) {
          send(this.ws, { type: "pad_info", kind: "generic", id: "keyboard+mouse" });
        }
      }
      this.padName = "keyboard+mouse";
      const now = performance.now();
      if (now - this.padWindowStart >= 1000) {
        this.lastPadHz = this.padSent;
        this.cb.onPadStats?.(this.padSent, "keyboard+mouse");
        this.padSent = 0;
        this.padWindowStart = now;
      }
      return;
    }
    // Tell the host which pad family this is. PadFrame is normalised by the
    // Gamepad API, so the host cannot infer it from input — without this it
    // binds the emulator to whatever was configured last, which drops every
    // button when that device is not the one in the player's hands.
    if (
      gp.id !== this.padInfoSent ||
      performance.now() - this.padInfoLastSentAt > CouchlinkPlayer.PAD_INFO_HEARTBEAT_MS
    ) {
      this.padInfoSent = gp.id;
      this.padInfoLastSentAt = performance.now();
      const kind = controllerKind(gp.id);
      if (this.ws?.readyState === WebSocket.OPEN) {
        send(this.ws, { type: "pad_info", kind, id: gp.id });
        clog("signal → pad_info", `${kind} (${gp.id})`);
      }
    }
    this.padName = gp.id;
    this.seq = (this.seq + 1) >>> 0;
    const state: PadState = fromBrowserGamepad(gp, this.seq);
    this.emitPad(state);
    const now = performance.now();
    if (now - this.padWindowStart >= 1000) {
      this.lastPadHz = this.padSent;
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
        if (msg.slot) this.cb.onRegistered?.(msg.slot);
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
        try {
          const epoch = msg.epoch ?? 0;
          await this.applyRemoteOffer(msg.sdp, epoch);
        } catch (e) {
          cerror("offer handling failed", e);
          this.cb.onState("error", `WebRTC offer failed: ${e}`);
        }
        break;
      }
      case "ice_candidate":
        if (this.pc) {
          try {
            await this.pc.addIceCandidate({
              candidate: msg.candidate,
              sdpMid: msg.sdpMid ?? undefined,
              sdpMLineIndex: msg.sdpMLineIndex ?? undefined,
            });
            clog("addIceCandidate ok", msg.candidate?.slice(0, 60));
          } catch (e) {
            cwarn("addIceCandidate failed", e, msg.candidate?.slice(0, 80));
          }
        } else {
          cwarn("ice_candidate before RTCPeerConnection exists");
        }
        break;
      case "stream_info":
        clog("stream_info", msg);
        this.cb.onStreamInfo?.(msg);
        break;
      case "host_stats":
        this.cb.onHostStats?.(msg);
        break;
      case "players_status":
        this.cb.onPlayersStatus?.(msg.occupied, msg.max);
        break;
      case "peer_left":
        // slot 0 means the host itself left; any other slot is a fellow
        // player leaving, now also broadcast here so a controller debug view
        // can drop their pad info — that must NOT be treated as our own host
        // connection dying.
        if (!msg.slot) {
          this.cb.onState("waiting_host", "Host disconnected");
          this.resetPeer();
        } else {
          this.cb.onPlayerLeft?.(msg.slot);
        }
        break;
      case "player_pad_info":
        this.cb.onPlayerPadInfo?.(msg.slot, msg.kind, msg.id ?? "");
        break;
      case "pong":
        break;
    }
  }
}
