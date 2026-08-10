//! WebCodecs H.264 decode → desynchronized canvas (DataChannel path).
//!
//! Bypasses WebRTC media / jitter buffer entirely. Requires a secure context
//! (https, localhost, or 127.0.0.1).

import { clog, cwarn } from "./log";
import type { VideoAccessUnit } from "./clvd";
import {
  annexBHasIdr,
  annexBToLengthPrefixed,
  buildAvcC,
  codecStringFromSps,
  extractParamSets,
} from "./h264Avc";

export function canUseWebCodecs(): boolean {
  return (
    typeof window !== "undefined" &&
    window.isSecureContext === true &&
    typeof VideoDecoder === "function" &&
    typeof EncodedVideoChunk === "function" &&
    typeof VideoFrame !== "undefined" &&
    typeof HTMLCanvasElement !== "undefined"
  );
}

/**
 * Label the likely bottleneck from the numbers this decoder can actually see.
 *
 * Deliberately coarse — this is a first triage step, not a diagnosis. `dropped`
 * counts frames this decoder discarded (stale/out of order), which on this
 * unordered channel is the client-visible symptom of a packet loss or a host
 * stall; it cannot tell those two apart from here, hence "consider" rather
 * than a firm verdict.
 */
export function webcodecsDiagnosis(
  presentFps: number,
  decodeMsAvg: number,
  dropped: number
): string {
  // A 60fps stream gives ~16.7ms per frame; decode competing with paint and
  // compositing eating half that budget is a real local bottleneck, not noise.
  if (decodeMsAvg > 8) {
    return `decode-bound (${decodeMsAvg.toFixed(1)}ms/frame on this device)`;
  }
  if (dropped > 0 && presentFps < 45) {
    return "frames arriving incomplete — possible network loss (host: try COUCHLINK_FEC=1)";
  }
  if (presentFps < 45) {
    return "low frame rate with fast local decode — host or network side, not this device";
  }
  return "healthy";
}

export type WebCodecsStats = {
  mode: "webcodecs";
  presentFps: number;
  dropped: number;
  width: number;
  height: number;
  decodeMs: number;
};

/**
 * Decodes H.264 access units (Annex-B on the wire → AVCC for WebCodecs)
 * and paints immediately to canvas.
 */
export class WebCodecsCanvasView {
  private decoder: VideoDecoder | null = null;
  private ctx: CanvasRenderingContext2D | null = null;
  private configured = false;
  private waitingKeyframe = true;
  private painted = 0;
  private paintedTotal = 0;
  private dropped = 0;
  private decodeMsAccum = 0;
  private windowStart = 0;
  private lastW = 0;
  private lastH = 0;
  private onStats: ((s: WebCodecsStats) => void) | null = null;
  private onNeedKeyframe: (() => void) | null = null;
  private lastPli = 0;
  private description: Uint8Array | null = null;
  private codec = "avc1.4D0028";
  private running = false;

  constructor(private canvas: HTMLCanvasElement) {}

  setStatsHandler(cb: ((s: WebCodecsStats) => void) | null) {
    this.onStats = cb;
  }

  setKeyframeHandler(cb: (() => void) | null) {
    this.onNeedKeyframe = cb;
  }

  /** True once at least one frame has been painted. */
  hasPainted(): boolean {
    return this.paintedTotal > 0;
  }

  isRunning(): boolean {
    return this.running;
  }

  start(): boolean {
    if (this.running && this.decoder && this.decoder.state !== "closed") {
      return true;
    }
    this.stop();
    if (!canUseWebCodecs()) return false;

    const ctx = this.canvas.getContext("2d", {
      alpha: false,
      desynchronized: true,
    } as CanvasRenderingContext2DSettings);
    if (!ctx) {
      cwarn("webcodecs canvas: 2d context unavailable");
      return false;
    }
    ctx.imageSmoothingEnabled = true;
    ctx.imageSmoothingQuality = "high";
    this.ctx = ctx;
    this.waitingKeyframe = true;
    this.configured = false;
    this.description = null;
    this.painted = 0;
    this.paintedTotal = 0;
    this.dropped = 0;
    this.decodeMsAccum = 0;
    this.windowStart = performance.now();

    if (!this.createDecoder()) {
      this.stop();
      return false;
    }
    this.running = true;
    clog("webcodecs canvas ready", {
      secureContext: window.isSecureContext,
      desynchronized:
        (
          ctx as CanvasRenderingContext2D & {
            getContextAttributes?: () => { desynchronized?: boolean };
          }
        ).getContextAttributes?.()?.desynchronized ?? "unknown",
    });
    return true;
  }

  private createDecoder(): boolean {
    try {
      this.decoder = new VideoDecoder({
        output: (frame) => {
          this.paint(frame);
          frame.close();
        },
        error: (e) => {
          cwarn("VideoDecoder error", String(e));
          this.resetForKeyframe();
        },
      });
      return true;
    } catch (e) {
      cwarn("VideoDecoder construct failed", String(e));
      this.decoder = null;
      return false;
    }
  }

  /** Close and rebuild decoder after a fatal decode error — keep canvas/ctx. */
  private resetForKeyframe() {
    this.waitingKeyframe = true;
    this.configured = false;
    try {
      this.decoder?.close();
    } catch {
      /* ignore */
    }
    this.decoder = null;
    this.createDecoder();
    this.requestKeyframe();
  }

  push(au: VideoAccessUnit) {
    const dec = this.decoder;
    if (!dec || dec.state === "closed") return;

    const keyframe = au.keyframe || annexBHasIdr(au.annexB);

    if (this.waitingKeyframe && !keyframe) {
      this.dropped += 1;
      this.requestKeyframe();
      return;
    }

    try {
      if (keyframe) {
        const params = extractParamSets(au.annexB);
        if (params) {
          this.description = buildAvcC(params.sps, params.pps);
          this.codec = codecStringFromSps(params.sps);
        }
      }

      if (!this.configured || dec.state === "unconfigured") {
        if (!keyframe || !this.description) {
          this.requestKeyframe();
          return;
        }
        if (dec.state === "configured") {
          // Shouldn't happen — recreate rather than double-configure.
          this.resetForKeyframe();
          return;
        }
        dec.configure({
          codec: this.codec,
          description: this.description,
          codedWidth: au.width || undefined,
          codedHeight: au.height || undefined,
          optimizeForLatency: true,
        });
        this.configured = true;
        clog("VideoDecoder configured", {
          codec: this.codec,
          w: au.width,
          h: au.height,
          annexB: au.annexB.byteLength,
        });
      }

      if (dec.decodeQueueSize > 2) {
        // Prefer newest over catch-up — drop this AU; keep decoder configured.
        this.dropped += 1;
        this.waitingKeyframe = true;
        this.requestKeyframe();
        return;
      }

      const avcc = annexBToLengthPrefixed(au.annexB, { omitParamSets: true });
      if (!avcc.length) {
        this.dropped += 1;
        return;
      }

      const t0 = performance.now();
      const chunk = new EncodedVideoChunk({
        type: keyframe ? "key" : "delta",
        timestamp: au.seq * 16_666,
        data: avcc,
      });
      dec.decode(chunk);
      this.decodeMsAccum += performance.now() - t0;
      this.waitingKeyframe = false;
    } catch (e) {
      cwarn("decode push failed", String(e));
      this.resetForKeyframe();
    }
  }

  private requestKeyframe() {
    const now = performance.now();
    if (now - this.lastPli < 200) return;
    this.lastPli = now;
    this.onNeedKeyframe?.();
  }

  private paint(frame: VideoFrame) {
    const ctx = this.ctx;
    if (!ctx) return;
    const w = frame.displayWidth || frame.codedWidth;
    const h = frame.displayHeight || frame.codedHeight;
    if (w > 0 && h > 0 && (this.canvas.width !== w || this.canvas.height !== h)) {
      this.canvas.width = w;
      this.canvas.height = h;
      this.lastW = w;
      this.lastH = h;
    }
    ctx.drawImage(frame, 0, 0);
    this.painted += 1;
    this.paintedTotal += 1;

    const now = performance.now();
    if (now - this.windowStart >= 1000) {
      const elapsed = (now - this.windowStart) / 1000;
      const n = Math.max(this.painted, 1);
      const presentFps = Math.round(this.painted / elapsed);
      const decodeMs = this.decodeMsAccum / n;
      // This is the real path for Chrome: WebCodecs + CLVD paints directly,
      // bypassing the RTP jitter buffer entirely. Every latency number logged
      // elsewhere tonight came from getStats() on the RTP receiver — a shadow
      // stream nobody was watching. This is the first one taken from the
      // pipeline actually on screen.
      clog("webcodecs stats", {
        presentFps,
        decodeMsAvg: Math.round(decodeMs * 10) / 10,
        dropped: this.dropped,
        width: this.lastW,
        height: this.lastH,
        diagnosis: webcodecsDiagnosis(presentFps, decodeMs, this.dropped),
      });
      this.onStats?.({
        mode: "webcodecs",
        presentFps,
        dropped: this.dropped,
        width: this.lastW,
        height: this.lastH,
        decodeMs,
      });
      this.painted = 0;
      this.dropped = 0;
      this.decodeMsAccum = 0;
      this.windowStart = now;
    }
  }

  stop() {
    this.running = false;
    try {
      this.decoder?.close();
    } catch {
      /* already closed */
    }
    this.decoder = null;
    this.ctx = null;
    this.configured = false;
    this.waitingKeyframe = true;
    this.description = null;
  }
}
