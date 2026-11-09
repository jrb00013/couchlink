/**
 * Seat oracle v0 runner — samples a canvas off the paint path, runs seam
 * detection (sync or optional ONNX Worker), and refines seat mapping via motion.
 */
import { assignSeatRegions, detectSeamsSync } from "./splitDetector";
import { detectSeams, splitOnnxEnabled } from "./splitOnnx";
import type { SplitDetection } from "./splitTypes";

const DOWNSCALE_W = 160;
const DOWNSCALE_H = 90;

export type OracleResult = SplitDetection & { seatRegion: number[] };

/** Grab RGBA from a source canvas at low res for oracle (never on encode path). */
export function sampleCanvasRgba(
  source: HTMLCanvasElement,
): { width: number; height: number; rgba: Uint8ClampedArray } | null {
  if (source.width < 2 || source.height < 2) return null;
  const c = document.createElement("canvas");
  c.width = DOWNSCALE_W;
  c.height = DOWNSCALE_H;
  const ctx = c.getContext("2d", { willReadFrequently: true });
  if (!ctx) return null;
  ctx.drawImage(source, 0, 0, DOWNSCALE_W, DOWNSCALE_H);
  const img = ctx.getImageData(0, 0, DOWNSCALE_W, DOWNSCALE_H);
  return { width: DOWNSCALE_W, height: DOWNSCALE_H, rgba: img.data };
}

export async function runOracle(opts: {
  source: HTMLCanvasElement;
  activeSeatHint?: number | null;
  preferOnnx?: boolean;
}): Promise<OracleResult> {
  const sample = sampleCanvasRgba(opts.source);
  if (!sample) {
    return {
      layout: "none",
      regions: [{ x: 0, y: 0, w: 1, h: 1 }],
      score: 0,
      seatRegion: [0],
    };
  }
  let det: SplitDetection;
  if (opts.preferOnnx && splitOnnxEnabled()) {
    det = await detectSeams(sample.width, sample.height, sample.rgba);
  } else {
    det = detectSeamsSync(sample.width, sample.height, sample.rgba);
  }
  const motion = new Array(det.regions.length).fill(0);
  const seatRegion = assignSeatRegions(
    det.regions.length,
    motion,
    opts.activeSeatHint ?? null,
  );
  return { ...det, seatRegion };
}
