//! Low-latency browser present path: MediaStreamTrack → VideoFrame → canvas.
//!
//! `<video srcObject=stream>` adds its own present queue on top of WebRTC's
//! jitter buffer. Pulling frames with MediaStreamTrackProcessor (maxBufferSize
//! 1) and drawing to a `desynchronized` canvas skips that queue and paints the
//! newest frame as soon as it arrives.

import { clog, cwarn } from "./log";

export function canUseLowLatencyCanvas(): boolean {
  return (
    typeof MediaStreamTrackProcessor === "function" &&
    typeof VideoFrame !== "undefined" &&
    typeof HTMLCanvasElement !== "undefined"
  );
}

export type PresentStats = {
  mode: "canvas" | "video";
  presentFps: number;
  dropped: number;
  width: number;
  height: number;
  /** Read → paint on this device (RTP has no host stamp). */
  ageMs: number;
};

/** Fired when a frame is painted — for age_echo (stamp_us 0 on RTP path). */
export type CanvasPaintedAge = {
  seq: number;
  recvMs: number;
  paintMs: number;
};

/**
 * Paints the newest decoded WebRTC frame to a canvas with minimal buffering.
 * Fallback to `<video>` is the caller's job when this returns false.
 */
export class LowLatencyCanvasView {
  private abort: AbortController | null = null;
  private ctx: CanvasRenderingContext2D | null = null;
  private painted = 0;
  private dropped = 0;
  private windowStart = 0;
  private lastW = 0;
  private lastH = 0;
  private lastAgeMs = 0;
  private paintSeq = 0;
  private onStats: ((s: PresentStats) => void) | null = null;
  private onPainted: ((a: CanvasPaintedAge) => void) | null = null;
  private onPumpDied: (() => void) | null = null;
  /** MSTC delivered frames but canvas stays ink-black — fall back to <video>. */
  private onBlackPresent: (() => void) | null = null;
  private blackStreak = 0;
  /** Bumped by every `start()`/`stop()` so a pump that dies after the caller
   * already moved on (new track, or stopped) does not restart a stale one. */
  private generation = 0;
  private restarts = 0;

  constructor(private canvas: HTMLCanvasElement) {}

  setStatsHandler(cb: ((s: PresentStats) => void) | null) {
    this.onStats = cb;
  }

  setPaintedHandler(cb: ((a: CanvasPaintedAge) => void) | null) {
    this.onPainted = cb;
  }

  /** Called when the pump could not be revived after its own retries — the
   * caller (App) should re-attach a fresh stream rather than leave the
   * canvas frozen on its last frame with no page refresh. */
  setPumpDiedHandler(cb: (() => void) | null) {
    this.onPumpDied = cb;
  }

  /** Fired when the canvas is receiving frames but stays near-black — MSTC
   * quirk on some Chromium forks. Caller should switch to <video srcObject>. */
  setBlackPresentHandler(cb: (() => void) | null) {
    this.onBlackPresent = cb;
  }

  async start(track: MediaStreamTrack): Promise<boolean> {
    this.stop();
    if (!canUseLowLatencyCanvas() || track.kind !== "video") {
      return false;
    }
    this.restarts = 0;
    return this.startInternal(track, ++this.generation);
  }

  private async startInternal(track: MediaStreamTrack, gen: number): Promise<boolean> {
    try {
      if ("contentHint" in track) {
        // Prefer spatial detail over pure motion — UI text stays readable on LAN
        // without adding a present queue (still desynchronized canvas).
        track.contentHint = "detail";
      }

      const ctx = this.canvas.getContext("2d", {
        alpha: false,
        desynchronized: true,
      } as CanvasRenderingContext2DSettings);
      if (!ctx) {
        cwarn("low-latency canvas: 2d context unavailable");
        return false;
      }
      ctx.imageSmoothingEnabled = true;
      ctx.imageSmoothingQuality = "high";
      this.ctx = ctx;

      const attrs = (
        ctx as CanvasRenderingContext2D & {
          getContextAttributes?: () => { desynchronized?: boolean };
        }
      ).getContextAttributes?.();
      clog("low-latency canvas start", {
        desynchronized: attrs?.desynchronized ?? "unknown",
        trackId: track.id,
      });

      const processor = new MediaStreamTrackProcessor({
        track,
        maxBufferSize: 1,
      });
      const reader: ReadableStreamDefaultReader<VideoFrame> =
        processor.readable.getReader();
      const abort = new AbortController();
      this.abort = abort;
      this.painted = 0;
      this.dropped = 0;
      this.lastAgeMs = 0;
      this.paintSeq = 0;
      this.windowStart = performance.now();

      const pump = async () => {
        // Distinct from an intentional stop() (aborted) — this is the pump
        // ending on its own (reader threw, or the stream reported `done`
        // without anyone calling stop()). Left unhandled, that used to
        // freeze the canvas on its last frame forever — no more paints, no
        // stats callback, and no signal to the caller that anything was
        // wrong (the black/frozen screen Joel had to refresh to clear).
        let diedUnexpectedly = false;
        try {
          while (!abort.signal.aborted) {
            const { value, done } = await reader.read();
            if (done || !value) {
              diedUnexpectedly = !abort.signal.aborted;
              break;
            }
            // Paint immediately — waiting for rAF would add up to one display frame.
            const frame: VideoFrame = value;
            const recvMs = performance.now();
            this.paint(frame, recvMs);
            frame.close();
          }
        } catch (e) {
          if (!abort.signal.aborted) {
            diedUnexpectedly = true;
            cwarn("low-latency canvas read ended", String(e));
          }
        } finally {
          try {
            reader.releaseLock();
          } catch {
            /* already locked/closed */
          }
        }
        if (diedUnexpectedly && gen === this.generation) {
          this.recoverPump(track, gen);
        }
      };

      void pump();
      return true;
    } catch (e) {
      cwarn("low-latency canvas failed to start", String(e));
      this.stop();
      return false;
    }
  }

  /** Bounded self-heal after the pump dies without `stop()` being called.
   * Most hiccups (a transient decoder error, a brief track mute around a
   * forced IDR) clear within one retry; if they don't, hand back to the
   * caller so it can re-attach a fresh stream instead of the canvas sitting
   * frozen with nothing painting until someone reloads the page. */
  private recoverPump(track: MediaStreamTrack, gen: number) {
    if (this.restarts >= 5) {
      cwarn("low-latency canvas pump died repeatedly — asking caller to re-attach");
      this.onPumpDied?.();
      return;
    }
    this.restarts += 1;
    const attempt = this.restarts;
    const delayMs = Math.min(150 * attempt, 1000);
    cwarn(`low-latency canvas pump died — restarting (attempt ${attempt}/5)`);
    window.setTimeout(() => {
      if (gen !== this.generation) return; // stop()/start() moved on already
      void this.startInternal(track, gen).then((ok) => {
        if (!ok && gen === this.generation) {
          this.recoverPump(track, gen);
        }
      });
    }, delayMs);
  }

  private paint(frame: VideoFrame, recvMs: number) {
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
    const paintMs = performance.now();
    const ageMs = Math.max(0, paintMs - recvMs);
    this.lastAgeMs = ageMs;
    this.paintSeq = (this.paintSeq + 1) >>> 0;
    // Sample ~4 Hz so pad DC is not flooded (host skips stamp_us=0 for glass age,
    // but records recv→paint as present-path age).
    if (this.paintSeq % 15 === 1) {
      this.onPainted?.({ seq: this.paintSeq, recvMs, paintMs });
    }
    // Detect MSTC "frames" that are ink-black while the track is live — seen as
    // a frozen black stage on some Chromium forks. Fall back to <video>.
    if (this.paintSeq % 20 === 0 && w >= 32 && h >= 32) {
      this.sampleBlackPresent(ctx, w, h);
    }

    const now = paintMs;
    if (now - this.windowStart >= 1000) {
      const elapsed = (now - this.windowStart) / 1000;
      this.onStats?.({
        mode: "canvas",
        presentFps: Math.round(this.painted / elapsed),
        dropped: this.dropped,
        width: this.lastW,
        height: this.lastH,
        ageMs: this.lastAgeMs,
      });
      this.painted = 0;
      this.dropped = 0;
      this.windowStart = now;
    }
  }

  /** Near-zero luma across a few center samples for several checks ⇒ black present. */
  private sampleBlackPresent(
    ctx: CanvasRenderingContext2D,
    w: number,
    h: number
  ) {
    try {
      const cx = (w / 2) | 0;
      const cy = (h / 2) | 0;
      const sample = ctx.getImageData(Math.max(0, cx - 2), Math.max(0, cy - 2), 4, 4);
      let sum = 0;
      const n = sample.data.length / 4;
      for (let i = 0; i < sample.data.length; i += 4) {
        sum += sample.data[i]! + sample.data[i + 1]! + sample.data[i + 2]!;
      }
      const avg = sum / (n * 3);
      if (avg < 2.5) {
        this.blackStreak += 1;
        // ~8 checks × 20 paints ≈ 160 frames (~1–2s at 90–120fps)
        if (this.blackStreak >= 8) {
          this.blackStreak = 0;
          cwarn("low-latency canvas staying black — asking caller for <video> fallback");
          this.onBlackPresent?.();
        }
      } else {
        this.blackStreak = 0;
      }
    } catch {
      // desynchronized / tainted canvas — cannot sample; leave present alone
    }
  }

  stop() {
    this.generation += 1;
    this.abort?.abort();
    this.abort = null;
    this.ctx = null;
    this.blackStreak = 0;
  }
}
