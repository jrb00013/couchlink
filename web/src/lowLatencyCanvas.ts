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
  private onStats: ((s: PresentStats) => void) | null = null;

  constructor(private canvas: HTMLCanvasElement) {}

  setStatsHandler(cb: ((s: PresentStats) => void) | null) {
    this.onStats = cb;
  }

  async start(track: MediaStreamTrack): Promise<boolean> {
    this.stop();
    if (!canUseLowLatencyCanvas() || track.kind !== "video") {
      return false;
    }

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
      this.windowStart = performance.now();

      const pump = async () => {
        try {
          while (!abort.signal.aborted) {
            const { value, done } = await reader.read();
            if (done || !value) break;
            // Paint immediately — waiting for rAF would add up to one display frame.
            const frame: VideoFrame = value;
            this.paint(frame);
            frame.close();
          }
        } catch (e) {
          if (!abort.signal.aborted) {
            cwarn("low-latency canvas read ended", String(e));
          }
        } finally {
          try {
            reader.releaseLock();
          } catch {
            /* already locked/closed */
          }
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
      this.onStats?.({
        mode: "canvas",
        presentFps: Math.round(this.painted / elapsed),
        dropped: this.dropped,
        width: this.lastW,
        height: this.lastH,
      });
      this.painted = 0;
      this.dropped = 0;
      this.windowStart = now;
    }
  }

  stop() {
    this.abort?.abort();
    this.abort = null;
    this.ctx = null;
  }
}
