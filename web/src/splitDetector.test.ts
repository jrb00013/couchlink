import { describe, expect, it } from "vitest";
import { detectSeamsSync } from "./splitDetector";

function fillSolid(
  width: number,
  height: number,
  r: number,
  g: number,
  b: number,
): Uint8ClampedArray {
  const rgba = new Uint8ClampedArray(width * height * 4);
  for (let i = 0; i < width * height; i++) {
    const o = i * 4;
    rgba[o] = r;
    rgba[o + 1] = g;
    rgba[o + 2] = b;
    rgba[o + 3] = 255;
  }
  return rgba;
}

/** Left/right halves with a sharp vertical seam in the middle band. */
function syntheticVerticalSeam(width: number, height: number): Uint8ClampedArray {
  const rgba = new Uint8ClampedArray(width * height * 4);
  const mid = Math.floor(width / 2);
  for (let y = 0; y < height; y++) {
    for (let x = 0; x < width; x++) {
      const dark = x < mid;
      const o = (y * width + x) * 4;
      rgba[o] = dark ? 20 : 220;
      rgba[o + 1] = dark ? 20 : 220;
      rgba[o + 2] = dark ? 20 : 220;
      rgba[o + 3] = 255;
    }
  }
  return rgba;
}

/** Top/bottom halves with a horizontal seam in the middle band. */
function syntheticHorizontalSeam(width: number, height: number): Uint8ClampedArray {
  const rgba = new Uint8ClampedArray(width * height * 4);
  const mid = Math.floor(height / 2);
  for (let y = 0; y < height; y++) {
    for (let x = 0; x < width; x++) {
      const dark = y < mid;
      const o = (y * width + x) * 4;
      rgba[o] = dark ? 15 : 200;
      rgba[o + 1] = dark ? 15 : 200;
      rgba[o + 2] = dark ? 15 : 200;
      rgba[o + 3] = 255;
    }
  }
  return rgba;
}

describe("detectSeamsSync", () => {
  const w = 160;
  const h = 90;

  it("returns none for uniform frame", () => {
    const rgba = fillSolid(w, h, 128, 128, 128);
    const d = detectSeamsSync(w, h, rgba);
    expect(d.layout).toBe("none");
    expect(d.regions).toHaveLength(1);
    expect(d.score).toBe(0);
  });

  it("detects a vertical split", () => {
    const rgba = syntheticVerticalSeam(w, h);
    const d = detectSeamsSync(w, h, rgba);
    expect(["vertical", "grid2x2"]).toContain(d.layout);
    expect(d.regions.length).toBeGreaterThanOrEqual(2);
    expect(d.score).toBeGreaterThan(2.5);
  });

  it("detects a horizontal split", () => {
    const rgba = syntheticHorizontalSeam(w, h);
    const d = detectSeamsSync(w, h, rgba);
    expect(d.layout).toBe("horizontal");
    expect(d.regions).toHaveLength(2);
    expect(d.score).toBeGreaterThan(2.5);
  });
});
