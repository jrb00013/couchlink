import { encodeClpd, fromBrowserGamepad, PAD_CHANNEL, type PadState } from "./clpd";
import {
  ClvdAssembler,
  decodeClvdFragment,
  PLI_BYTES,
  VIDEO_CHANNEL,
  type VideoAccessUnit,
} from "./clvd";
import { controllerKind } from "./controllerKind";
import { clog, cerror, cwarn } from "./log";
import { jitterWindow } from "./latencyStats";
import { send, type SignalMessage } from "./proto";
import { canUseWebCodecs } from "./webCodecsCanvas";

export type ConnectionState =
  | "disconnected"
  | "connecting"
  | "registering"
  | "waiting_host"
  | "negotiating"
  | "connected"
  | "error";

export type PresentPath = "webcodecs" | "rtp";

export interface PlayerCallbacks {
  onState: (s: ConnectionState, detail?: string) => void;
  onVideo: (stream: MediaStream) => void;
  /** Annex-B access units from the unordered `video` DataChannel. */
  onVideoAccessUnit?: (au: VideoAccessUnit) => void;
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
  onPadStats?: (hz: number, name: string) => void;
}

const SESSION_NOT_FOUND_RETRIES = 12;
const SESSION_NOT_FOUND_DELAY_MS = 750;
const MEDIA_RECOVER_DELAY_MS = 5000;

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
  private sessionRetries = 0;
  private pending: { sid: string; pin: string } | null = null;
  private seq = 0;
  private padSent = 0;
  private padWindowStart = 0;
  private padName = "none";
  /** Last Gamepad.id announced to the host, so pad_info is sent only on change. */
  private padInfoSent = "";
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
        stats.forEach((r: any) => {
          if (r.type !== "inbound-rtp" || r.kind !== "video") return;
          const delay = r.jitterBufferDelay ?? 0;
          const count = r.jitterBufferEmittedCount ?? 0;
          const decoded = r.framesDecoded ?? 0;
          const prev = this.lastStats;
          this.lastStats = { delay, count, decoded };
          if (!prev || count === prev.count) return;
          const window = jitterWindow(
            {
              jitterBufferDelay: prev.delay,
              jitterBufferEmittedCount: prev.count,
              framesDecoded: prev.decoded,
              framesDropped: 0,
            },
            {
              jitterBufferDelay: delay,
              jitterBufferEmittedCount: count,
              framesDecoded: decoded,
              framesDropped: r.framesDropped ?? 0,
            },
            2
          );
          if (!window) return;
          // Chrome will grow the JB after packet jitter; pin it back every poll.
          this.pinJitterBuffer();
          clog("video stats", {
            jitterBufferMs: Math.round(window.jitterBufferMs),
            decodeFps: Math.round(window.decodeFps),
            framesDropped: window.framesDropped,
            frameHeight: r.frameHeight,
            pauseCount: r.pauseCount,
            freezeCount: r.freezeCount,
            totalFreezesDuration: r.totalFreezesDuration,
            jbTarget: this.videoReceiver?.jitterBufferTarget ?? null,
          });
        });
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
    if (this.heartbeatTimer) clearInterval(this.heartbeatTimer);
    if (this.statsTimer) clearInterval(this.statsTimer);
    if (this.padTimer) cancelAnimationFrame(this.padTimer);
    this.connectTimer = null;
    this.sessionRetryTimer = null;
    this.mediaRecoverTimer = null;
    this.heartbeatTimer = null;
    this.statsTimer = null;
    this.lastStats = null;
    this.padTimer = null;
    this.resetPeer();
    this.ws?.close();
    this.ws = null;
  }

  /** Drop WebRTC only; keep signaling socket if open. */
  private resetPeer() {
    if (this.padTimer) cancelAnimationFrame(this.padTimer);
    this.padTimer = null;
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
    }, MEDIA_RECOVER_DELAY_MS);
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
      if (pc.connectionState === "failed") {
        cwarn("WebRTC connection failed — check ICE / firewall / WSL IP in signaling URL");
        this.scheduleMediaRecover("connection failed");
      } else if (pc.connectionState === "connected") {
        this.mediaHealthy = this.gotVideoTrack;
        if (this.mediaRecoverTimer) {
          clearTimeout(this.mediaRecoverTimer);
          this.mediaRecoverTimer = null;
        }
      }
    };
    pc.oniceconnectionstatechange = () => {
      clog("pc.iceConnectionState", pc.iceConnectionState);
      if (pc.iceConnectionState === "failed") {
        cwarn("ICE problem", pc.iceConnectionState);
        this.scheduleMediaRecover("ICE failed");
      } else if (pc.iceConnectionState === "disconnected") {
        cwarn("ICE disconnected (may recover on its own)", pc.iceConnectionState);
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
        clog("RTP track received — held for fallback (WebCodecs/CLVD preferred)");
        this.cb.onVideo(stream);
        return;
      }
      this.cb.onPresentPath?.("rtp");
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
        this.cb.onPresentPath?.(
          "webcodecs",
          "CLVD DataChannel + WebCodecs (no RTP jitter buffer)"
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
        this.cb.onPresentPath?.(
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
      this.cb.onVideoAccessUnit?.(au);
    };
    ch.onclose = () => clog("video datachannel closed");
    ch.onerror = (e) => cerror("video datachannel error", e);
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
    this.cb.onPresentPath?.("rtp", "WebCodecs fallback");
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
    // Tell the host which pad family this is. PadFrame is normalised by the
    // Gamepad API, so the host cannot infer it from input — without this it
    // binds the emulator to whatever was configured last, which drops every
    // button when that device is not the one in the player's hands.
    if (gp.id !== this.padInfoSent) {
      this.padInfoSent = gp.id;
      const kind = controllerKind(gp.id);
      if (this.ws?.readyState === WebSocket.OPEN) {
        send(this.ws, { type: "pad_info", kind, id: gp.id });
        clog("signal → pad_info", `${kind} (${gp.id})`);
      }
    }
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
      case "peer_left":
        this.cb.onState("waiting_host", "Host disconnected");
        this.resetPeer();
        break;
      case "pong":
        break;
    }
  }
}
