//! WebCodecs H.264 decode → latest-frame-wins canvas (DataChannel path).
//!
//! Presentation is real-time, not ordered media: never wait for an older frame
//! just because it belongs to the sequence. Decoder output parks one pending
//! VideoFrame; requestAnimationFrame paints only the newest.

import { clog, cwarn } from "./log";
import type { VideoAccessUnit } from "./clvd";
import {
  annexBHasIdr,
  annexBToLengthPrefixed,
  buildAvcC,
  codecStringFromSps,
  extractParamSets,
} from "./h264Avc";
import {
  ageBand,
  AGE_DROP_MS,
  shouldReplacePending,
  shouldSkipDecode,
  type AgeBand,
} from "./presentAge";

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
  /** Receive → present age of the last painted frame (ms). */
  ageMs: number;
  ageBand: AgeBand;
};

/** Fired when a frame is actually painted — for host age_echo. */
export type PaintedAge = {
  seq: number;
  stampUs: number;
  recvMs: number;
  paintMs: number;
};

type FrameMeta = {
  seq: number;
  stampUs: number;
  recvMs: number;
};

/** Microsecond-scale chunk timestamp from AU seq (matches EncodedVideoChunk). */
function chunkTimestampUs(seq: number): number {
  return seq * 16_666;
}

/**
 * Decodes H.264 access units (Annex-B on the wire → AVCC for WebCodecs)
 * and presents the newest decoded frame on rAF (latest-frame-wins).
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
  private lastAgeMs = 0;
  private lastAgeBand: AgeBand = "ok";
  private onStats: ((s: WebCodecsStats) => void) | null = null;
  private onNeedKeyframe: (() => void) | null = null;
  private onFirstPaint: (() => void) | null = null;
  private onStall: (() => void) | null = null;
  private onPainted: ((a: PaintedAge) => void) | null = null;
  private lastPli = 0;
  private lastPaintAt = 0;
  private stallTimer: number | null = null;
  /** After a first paint, this many ms without another means the GOP is dead. */
  private static readonly STALL_MS = 1500;
  private description: Uint8Array | null = null;
  private codec = "avc1.4D0028";
  private running = false;

  /** Newest decoded frame waiting for the compositor — older ones are closed. */
  private pending: VideoFrame | null = null;
  private pendingMeta: FrameMeta | null = null;
  private pendingTs: number | null = null;
  private raf = 0;

  /** AU metadata keyed by EncodedVideoChunk timestamp. */
  private metaByTs = new Map<number, FrameMeta>();

  constructor(private canvas: HTMLCanvasElement) {}

  setStatsHandler(cb: ((s: WebCodecsStats) => void) | null) {
    this.onStats = cb;
  }

  setKeyframeHandler(cb: (() => void) | null) {
    this.onNeedKeyframe = cb;
  }

  /** Fired once, the first time a decoded frame is painted on screen. */
  setFirstPaintHandler(cb: (() => void) | null) {
    this.onFirstPaint = cb;
  }

  /** Fired when we had picture and then went dark — show the live RTP canvas. */
  setStallHandler(cb: (() => void) | null) {
    this.onStall = cb;
  }

  /** Fired on each paint with timestamps for host age_echo. */
  setPaintedHandler(cb: ((a: PaintedAge) => void) | null) {
    this.onPainted = cb;
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
    this.lastAgeMs = 0;
    this.lastAgeBand = "ok";
    this.windowStart = performance.now();
    this.clearPending();
    this.metaByTs.clear();

    if (!this.createDecoder()) {
      this.stop();
      return false;
    }
    this.running = true;
    this.lastPaintAt = 0;
    if (this.stallTimer !== null) window.clearInterval(this.stallTimer);
    this.stallTimer = window.setInterval(() => this.checkStall(), 500);
    clog("webcodecs canvas ready (latest-frame-wins)", {
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
          this.parkDecoded(frame);
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
    this.clearPending();
    this.metaByTs.clear();
    try {
      this.decoder?.close();
    } catch {
      /* ignore */
    }
    this.decoder = null;
    this.createDecoder();
    this.requestKeyframe();
  }

  /**
   * Park the newest decoded frame; close any older pending frame.
   * Presentation happens on rAF — never drain a queue of old pictures.
   */
  private parkDecoded(frame: VideoFrame) {
    const ts = frame.timestamp;
    const meta = this.metaByTs.get(ts) ?? null;
    if (meta) this.metaByTs.delete(ts);
    // Drop orphaned meta entries so the map cannot grow without bound.
    if (this.metaByTs.size > 64) {
      const first = this.metaByTs.keys().next().value;
      if (first !== undefined) this.metaByTs.delete(first);
    }

    if (!shouldReplacePending(this.pendingTs, ts)) {
      frame.close();
      this.dropped += 1;
      return;
    }

    if (this.pending) {
      this.pending.close();
      this.dropped += 1;
    }
    this.pending = frame;
    this.pendingMeta = meta;
    this.pendingTs = ts;
    this.schedulePresent();
  }

  private schedulePresent() {
    if (this.raf) return;
    this.raf = requestAnimationFrame(() => {
      this.raf = 0;
      this.presentLatest();
    });
  }

  private presentLatest() {
    const frame = this.pending;
    if (!frame) return;
    const meta = this.pendingMeta;
    this.pending = null;
    this.pendingMeta = null;
    this.pendingTs = null;

    const paintMs = performance.now();
    const recvMs = meta?.recvMs ?? paintMs;
    const ageMs = Math.max(0, paintMs - recvMs);
    const band = ageBand(ageMs);

    // Age > DROP: still show the newest (freshness), but count as a drop signal
    // so the HUD/stats make the overshoot visible. Never wait for an older frame.
    if (band === "drop" || band === "emergency") {
      // Keep painting — interactive display prefers a late newest over a blank.
    }

    this.paint(frame, ageMs, band);
    frame.close();

    if (meta) {
      this.onPainted?.({
        seq: meta.seq,
        stampUs: meta.stampUs,
        recvMs: meta.recvMs,
        paintMs,
      });
    }
  }

  push(au: VideoAccessUnit, recvMs = performance.now()) {
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

      if (shouldSkipDecode(dec.decodeQueueSize, keyframe)) {
        // Decoder is backed up locally (slow paint/GPU, not a network issue).
        // Drop the frame to let it catch up, but do NOT set waitingKeyframe —
        // the decoder is still configured and the last keyframe is still valid.
        this.dropped += 1;
        return;
      }

      // If we already hold a pending frame older than the drop budget, prefer
      // not to enqueue more deltas until the presenter drains.
      if (
        this.pending &&
        this.pendingMeta &&
        !keyframe &&
        performance.now() - this.pendingMeta.recvMs > AGE_DROP_MS
      ) {
        this.dropped += 1;
        return;
      }

      const avcc = annexBToLengthPrefixed(au.annexB, { omitParamSets: true });
      if (!avcc.length) {
        this.dropped += 1;
        return;
      }

      const ts = chunkTimestampUs(au.seq);
      this.metaByTs.set(ts, { seq: au.seq, stampUs: au.stampUs, recvMs });

      const t0 = performance.now();
      const chunk = new EncodedVideoChunk({
        type: keyframe ? "key" : "delta",
        timestamp: ts,
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

  private paint(frame: VideoFrame, ageMs: number, band: AgeBand) {
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
    this.lastPaintAt = performance.now();
    this.lastAgeMs = ageMs;
    this.lastAgeBand = band;
    if (this.paintedTotal === 1) {
      this.onFirstPaint?.();
    }

    const now = performance.now();
    if (now - this.windowStart >= 1000) {
      const elapsed = (now - this.windowStart) / 1000;
      const n = Math.max(this.painted, 1);
      const presentFps = Math.round(this.painted / elapsed);
      const decodeMs = this.decodeMsAccum / n;
      clog("webcodecs stats", {
        presentFps,
        decodeMsAvg: Math.round(decodeMs * 10) / 10,
        ageMs: Math.round(this.lastAgeMs * 10) / 10,
        ageBand: this.lastAgeBand,
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
        ageMs: this.lastAgeMs,
        ageBand: this.lastAgeBand,
      });
      this.painted = 0;
      this.dropped = 0;
      this.decodeMsAccum = 0;
      this.windowStart = now;
    }
  }

  private clearPending() {
    if (this.raf) {
      cancelAnimationFrame(this.raf);
      this.raf = 0;
    }
    if (this.pending) {
      try {
        this.pending.close();
      } catch {
        /* ignore */
      }
      this.pending = null;
    }
    this.pendingMeta = null;
    this.pendingTs = null;
  }

  private checkStall() {
    if (!this.running || this.paintedTotal === 0 || this.lastPaintAt === 0) return;
    if (performance.now() - this.lastPaintAt < WebCodecsCanvasView.STALL_MS) return;
    cwarn("webcodecs stall — no paint, resetting decoder and showing live RTP");
    this.lastPaintAt = performance.now();
    this.resetForKeyframe();
    this.onStall?.();
  }

  stop() {
    this.running = false;
    if (this.stallTimer !== null) {
      window.clearInterval(this.stallTimer);
      this.stallTimer = null;
    }
    this.clearPending();
    this.metaByTs.clear();
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
