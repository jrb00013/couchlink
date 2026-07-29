//! WebCodecs H.264 decode → desynchronized canvas (DataChannel path).
//!
//! Bypasses WebRTC media / jitter buffer entirely. Requires a secure context
//! (https, localhost, or 127.0.0.1).

import { clog, cwarn } from "./log";
import type { VideoAccessUnit } from "./clvd";
import {
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
  private dropped = 0;
  private decodeMsAccum = 0;
  private windowStart = 0;
  private lastW = 0;
  private lastH = 0;
  private onStats: ((s: WebCodecsStats) => void) | null = null;
  private onNeedKeyframe: (() => void) | null = null;
  private lastPli = 0;
  private description: Uint8Array | null = null;
  private codec = "avc1.42E01F";

  constructor(private canvas: HTMLCanvasElement) {}

  setStatsHandler(cb: ((s: WebCodecsStats) => void) | null) {
    this.onStats = cb;
  }

  setKeyframeHandler(cb: (() => void) | null) {
    this.onNeedKeyframe = cb;
  }

  start(): boolean {
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
    this.ctx = ctx;
    this.waitingKeyframe = true;
    this.configured = false;
    this.description = null;
    this.painted = 0;
    this.dropped = 0;
    this.decodeMsAccum = 0;
    this.windowStart = performance.now();

    try {
      this.decoder = new VideoDecoder({
        output: (frame) => {
          this.paint(frame);
          frame.close();
        },
        error: (e) => {
          cwarn("VideoDecoder error", String(e));
          this.waitingKeyframe = true;
          this.configured = false;
          this.requestKeyframe();
        },
      });
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
    } catch (e) {
      cwarn("VideoDecoder construct failed", String(e));
      this.stop();
      return false;
    }
  }

  push(au: VideoAccessUnit) {
    const dec = this.decoder;
    if (!dec || dec.state === "closed") return;

    if (this.waitingKeyframe && !au.keyframe) {
      this.dropped += 1;
      this.requestKeyframe();
      return;
    }

    try {
      if (au.keyframe) {
        const params = extractParamSets(au.annexB);
        if (params) {
          this.description = buildAvcC(params.sps, params.pps);
          this.codec = codecStringFromSps(params.sps);
        }
      }

      if (!this.configured || dec.state === "unconfigured") {
        if (!au.keyframe || !this.description) {
          this.requestKeyframe();
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
        });
      }

      if (dec.decodeQueueSize > 2) {
        this.dropped += 1;
        this.waitingKeyframe = true;
        this.configured = false;
        this.requestKeyframe();
        return;
      }

      const avcc = annexBToLengthPrefixed(au.annexB);
      if (!avcc.length) {
        this.dropped += 1;
        return;
      }

      const t0 = performance.now();
      const chunk = new EncodedVideoChunk({
        type: au.keyframe ? "key" : "delta",
        timestamp: au.seq * 16_666,
        data: avcc,
      });
      dec.decode(chunk);
      this.decodeMsAccum += performance.now() - t0;
      this.waitingKeyframe = false;
    } catch (e) {
      cwarn("decode push failed", String(e));
      this.waitingKeyframe = true;
      this.configured = false;
      this.requestKeyframe();
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

    const now = performance.now();
    if (now - this.windowStart >= 1000) {
      const elapsed = (now - this.windowStart) / 1000;
      const n = Math.max(this.painted, 1);
      this.onStats?.({
        mode: "webcodecs",
        presentFps: Math.round(this.painted / elapsed),
        dropped: this.dropped,
        width: this.lastW,
        height: this.lastH,
        decodeMs: this.decodeMsAccum / n,
      });
      this.painted = 0;
      this.dropped = 0;
      this.decodeMsAccum = 0;
      this.windowStart = now;
    }
  }

  stop() {
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
