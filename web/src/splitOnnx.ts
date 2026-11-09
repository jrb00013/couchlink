/**
 * Optional ONNX seat oracle in a Worker. Falls back to heuristic detectSeamsSync
 * when no model is loaded — crop APIs in splitSeat.ts are unchanged.
 */
import type { SplitDetection } from "./splitTypes";
import { detectSeamsSync } from "./splitDetector";

let worker: Worker | null = null;
let modelUrl: string | null = null;

export function configureSplitOnnx(url: string | null): void {
  modelUrl = url;
  worker = null;
}

/** True when a Worker + model URL are configured (inference may still fall back). */
export function splitOnnxEnabled(): boolean {
  return modelUrl != null && modelUrl.length > 0;
}

function ensureWorker(): Worker | null {
  if (!splitOnnxEnabled()) return null;
  if (worker) return worker;
  try {
    worker = new Worker(new URL("./splitOnnx.worker.ts", import.meta.url), {
      type: "module",
    });
    worker.postMessage({ type: "load", url: modelUrl });
  } catch {
    worker = null;
  }
  return worker;
}

/**
 * Detect split layout; uses ONNX Worker when configured, else detectSeamsSync.
 */
export async function detectSeams(
  width: number,
  height: number,
  rgba: Uint8ClampedArray,
): Promise<SplitDetection> {
  const w = ensureWorker();
  if (!w) {
    return detectSeamsSync(width, height, rgba);
  }

  return new Promise((resolve) => {
    const buf = rgba.buffer.slice(
      rgba.byteOffset,
      rgba.byteOffset + rgba.byteLength,
    );
    const onMessage = (ev: MessageEvent) => {
      w.removeEventListener("message", onMessage);
      const data = ev.data as { ok?: boolean; detection?: SplitDetection };
      if (data?.ok && data.detection) {
        resolve(data.detection);
      } else {
        resolve(detectSeamsSync(width, height, rgba));
      }
    };
    w.addEventListener("message", onMessage);
    w.postMessage(
      { type: "infer", width, height, rgba: buf },
      [buf],
    );
  });
}
