/**
 * Seat oracle v0 — one-shot / periodic seam detection off the paint path.
 * Runs work in a Worker on a 160×90 downscale; never blocks rAF encode/decode.
 */
import type { NormRect } from "./proto";
import type { SplitDetection, SplitLayout } from "./splitTypes";

export type DetectRequest = {
  /** RGBA or ImageBitmap transferable — we use ImageData from caller. */
  width: number;
  height: number;
  /** RGBA bytes length width*height*4 */
  rgba: ArrayBuffer;
};

export function detectSeamsSync(
  width: number,
  height: number,
  rgba: Uint8ClampedArray,
): SplitDetection {
  const col = new Float64Array(width);
  const row = new Float64Array(height);
  for (let y = 0; y < height; y++) {
    let rs = 0;
    for (let x = 0; x < width; x++) {
      const i = (y * width + x) * 4;
      const luma = 0.2126 * rgba[i] + 0.7152 * rgba[i + 1] + 0.0722 * rgba[i + 2];
      col[x] += luma;
      rs += luma;
    }
    row[y] = rs / width;
  }
  for (let x = 0; x < width; x++) col[x] /= height;

  const x0 = Math.floor(width * 0.2);
  const x1 = Math.floor(width * 0.8);
  const y0 = Math.floor(height * 0.2);
  const y1 = Math.floor(height * 0.8);

  const scoreAxis = (
    means: Float64Array,
    start: number,
    end: number,
    midLo: number,
    midHi: number,
  ) => {
    let best = { idx: -1, score: 0 };
    for (let i = midLo; i < midHi; i++) {
      const g = Math.abs(means[i] - means[i - 1]);
      // median-ish: compare to neighborhood mean
      let neigh = 0;
      let n = 0;
      for (let k = Math.max(start, i - 4); k < Math.min(end, i + 5); k++) {
        if (k === i) continue;
        neigh += Math.abs(means[k] - means[k - 1] || 0);
        n++;
      }
      const med = n ? neigh / n : 1;
      const score = g / (med + 1e-3);
      if (score > best.score) best = { idx: i, score };
    }
    return best;
  };

  const vSeam = scoreAxis(col, 1, width, Math.floor(width * 0.35), Math.floor(width * 0.65));
  const hSeam = scoreAxis(row, 1, height, Math.floor(height * 0.35), Math.floor(height * 0.65));

  const FLOOR = 2.5;
  let layout: SplitLayout = "none";
  let regions: NormRect[] = [{ x: 0, y: 0, w: 1, h: 1 }];
  let score = 0;

  if (vSeam.score >= FLOOR && vSeam.score >= hSeam.score) {
    // expand gutter
    let L = vSeam.idx;
    let R = vSeam.idx;
    while (L > x0 && Math.abs(col[L] - col[L - 1]) < 8) L--;
    while (R < x1 && Math.abs(col[R] - col[R + 1]) < 8) R++;
    const mid = (L + R) / 2 / width;
    layout = "vertical";
    score = vSeam.score;
    regions = [
      { x: 0, y: 0, w: mid, h: 1 },
      { x: mid, y: 0, w: 1 - mid, h: 1 },
    ];
    // second seam on rows → 2x2
    if (hSeam.score >= FLOOR) {
      const midY = hSeam.idx / height;
      layout = "grid2x2";
      score = Math.min(vSeam.score, hSeam.score);
      regions = [
        { x: 0, y: 0, w: mid, h: midY },
        { x: mid, y: 0, w: 1 - mid, h: midY },
        { x: 0, y: midY, w: mid, h: 1 - midY },
        { x: mid, y: midY, w: 1 - mid, h: 1 - midY },
      ];
    }
  } else if (hSeam.score >= FLOOR) {
    const mid = hSeam.idx / height;
    layout = "horizontal";
    score = hSeam.score;
    regions = [
      { x: 0, y: 0, w: 1, h: mid },
      { x: 0, y: mid, w: 1, h: 1 - mid },
    ];
  }

  return { layout, regions, score };
}

/** Downscale VideoFrame/canvas to 160×90 and detect. */
export async function detectFromVideoFrame(
  source: CanvasImageSource,
  srcW: number,
  srcH: number,
): Promise<SplitDetection> {
  const w = 160;
  const h = 90;
  const c = document.createElement("canvas");
  c.width = w;
  c.height = h;
  const ctx = c.getContext("2d", { willReadFrequently: true });
  if (!ctx) return { layout: "none", regions: [{ x: 0, y: 0, w: 1, h: 1 }], score: 0 };
  ctx.drawImage(source, 0, 0, srcW, srcH, 0, 0, w, h);
  const img = ctx.getImageData(0, 0, w, h);
  return detectSeamsSync(w, h, img.data);
}

/**
 * Correlate pad activity hint with region motion (oracle v0 refinement).
 * `motionScores[i]` = recent motion energy in region i; `activeSeatHint` 1..=4.
 */
export function assignSeatRegions(
  regionCount: number,
  motionScores: number[],
  activeSeatHint: number | null,
): number[] {
  const n = Math.max(1, Math.min(4, regionCount));
  const seats = Array.from({ length: n }, (_, i) => i);
  if (activeSeatHint == null || activeSeatHint < 1 || activeSeatHint > n) {
    return seats;
  }
  // Put the hottest motion region on the active seat index (1-based → 0-based).
  let hot = 0;
  for (let i = 1; i < motionScores.length && i < n; i++) {
    if ((motionScores[i] ?? 0) > (motionScores[hot] ?? 0)) hot = i;
  }
  const seatIdx = activeSeatHint - 1;
  if (hot !== seatIdx && hot < n) {
    const out = seats.slice();
    out[seatIdx] = hot;
    out[hot] = seatIdx;
    return out;
  }
  return seats;
}
