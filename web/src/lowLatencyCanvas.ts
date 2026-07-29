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
 * Falls back is the caller's job when `canUseLowLatencyCanvas()` is false.
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
      // motion = prefer latency over resolution when the browser has to choose.
      if ("contentHint" in track) {
        track.contentHint = "motion";
      }

      const ctx = this.canvas.getContext("2d", {
        alpha: false,
        desynchronized: true,
        // willReadFrequently omitted — we only write.
      } as CanvasRenderingContext2DSettings);
      if (!ctx) {
        cwarn("low-latency canvas: 2d context unavailable");
        return false;
      }
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

      // Keep at most one undecoded/undrawn frame — older ones are latency we
      // will never get back. Chromium honours maxBufferSize on the constructor.
      const processor = new MediaStreamTrackProcessor({
        track,
        maxBufferSize: 1,
      });
      const reader = processor.readable.getReader();
      const abort = new AbortController();
      this.abort = abort;
      this.painted = 0;
      this.dropped = 0;
      this.windowStart = performance.now();

      const pump = async () => {
        let pending: VideoFrame | null = null;
        try {
          while (!abort.signal.aborted) {
            const { value, done } = await reader.read();
            if (done) break;
            if (!value) continue;

            // Always keep only the newest frame. If we somehow got ahead of
            // ourselves (draw was slow), close the stale one without painting.
            if (pending) {
              pending.close();
              this.dropped += 1;
            }
            pending = value;

            // Drain any already-queued frames so we present "now", not "then".
            // Controllers that support BYOB aren't required; try-read via
            // unlocked streams isn't available on all readers — instead we
            // paint immediately and rely on maxBufferSize: 1.
            const frame = pending;
            pending = null;
            this.paint(frame);
            frame.close();
          }
        } catch (e) {
          if (!abort.signal.aborted) {
            cwarn("low-latency canvas read ended", String(e));
          }
        } finally {
          pending?.close();
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
