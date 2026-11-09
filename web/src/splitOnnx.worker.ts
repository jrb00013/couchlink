/**
 * Stub Worker: ONNX runtime wiring is optional; always able to fall back.
 */
import { detectSeamsSync } from "./splitDetector";
import type { SplitDetection } from "./splitTypes";

self.onmessage = (ev: MessageEvent) => {
  const msg = ev.data as {
    type: string;
    url?: string;
    width?: number;
    height?: number;
    rgba?: ArrayBuffer;
  };

  if (msg.type === "load") {
    // Future: fetch msg.url and session.create — for v1 stub, ONNX stays unloaded.
    return;
  }

  if (msg.type === "infer" && msg.width && msg.height && msg.rgba) {
    const rgba = new Uint8ClampedArray(msg.rgba);
    const detection: SplitDetection = detectSeamsSync(msg.width, msg.height, rgba);
    self.postMessage({ ok: true, detection });
  }
};
