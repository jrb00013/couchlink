/**
 * Simulate old-school local split gameplay: two differently colored seat panels
 * with a dark gutter. Oracle must detect the seam; crop helpers must isolate
 * the remote seat without changing the master-off full-frame path.
 */
import { describe, expect, it } from "vitest";
import { detectSeamsSync, assignSeatRegions } from "./splitDetector";
import { cropForClient, parseSplitConfig } from "./splitSeat";

function paintVerticalSplit(
  w: number,
  h: number,
  mid: number,
): Uint8ClampedArray {
  const rgba = new Uint8ClampedArray(w * h * 4);
  const gutter = Math.max(2, Math.floor(w * 0.02));
  for (let y = 0; y < h; y++) {
    for (let x = 0; x < w; x++) {
      const i = (y * w + x) * 4;
      if (x >= mid - gutter / 2 && x < mid + gutter / 2) {
        // dark seam
        rgba[i] = 8;
        rgba[i + 1] = 8;
        rgba[i + 2] = 8;
      } else if (x < mid) {
        // P1 panel — bright green field (classic MK-ish left)
        rgba[i] = 40;
        rgba[i + 1] = 180;
        rgba[i + 2] = 60;
      } else {
        // P2 panel — blue track
        rgba[i] = 30;
        rgba[i + 1] = 80;
        rgba[i + 2] = 200;
      }
      rgba[i + 3] = 255;
    }
  }
  return rgba;
}

function paintHorizontalSplit(w: number, h: number, mid: number): Uint8ClampedArray {
  const rgba = new Uint8ClampedArray(w * h * 4);
  const gutter = Math.max(2, Math.floor(h * 0.02));
  for (let y = 0; y < h; y++) {
    for (let x = 0; x < w; x++) {
      const i = (y * w + x) * 4;
      if (y >= mid - gutter / 2 && y < mid + gutter / 2) {
        rgba[i] = 6;
        rgba[i + 1] = 6;
        rgba[i + 2] = 6;
      } else if (y < mid) {
        rgba[i] = 200;
        rgba[i + 1] = 40;
        rgba[i + 2] = 40;
      } else {
        rgba[i] = 40;
        rgba[i + 1] = 40;
        rgba[i + 2] = 200;
      }
      rgba[i + 3] = 255;
    }
  }
  return rgba;
}

describe("simulated split gameplay (oracle + crop)", () => {
  it("detects vertical 2P split and crops remote P2 to right seat", () => {
    const w = 160;
    const h = 90;
    const mid = 80;
    const rgba = paintVerticalSplit(w, h, mid);
    const det = detectSeamsSync(w, h, rgba);
    expect(det.layout).toBe("vertical");
    expect(det.regions.length).toBe(2);
    expect(det.score).toBeGreaterThan(2);

    const seats = assignSeatRegions(det.regions.length, [0, 10], 2);
    const cfg = parseSplitConfig({
      master_enabled: true,
      host_view_mode: "own",
      layout: det.layout,
      regions: det.regions,
      seat_region: seats,
    });
    const crop = cropForClient({
      masterEnabled: cfg.masterEnabled,
      viewMode: "own",
      regions: cfg.regions,
      seatRegion: cfg.seatRegion,
      mySlot: 1, // remote → seat P2
    });
    expect(crop).not.toBeNull();
    expect(crop!.x).toBeGreaterThan(0.35);
    expect(crop!.w).toBeLessThan(0.65);
  });

  it("detects horizontal 2P split", () => {
    const w = 160;
    const h = 90;
    const rgba = paintHorizontalSplit(w, h, 45);
    const det = detectSeamsSync(w, h, rgba);
    expect(det.layout).toBe("horizontal");
    expect(det.regions.length).toBe(2);
  });

  it("master off ignores detected regions (no regression path)", () => {
    const w = 160;
    const h = 90;
    const det = detectSeamsSync(w, h, paintVerticalSplit(w, h, 80));
    const crop = cropForClient({
      masterEnabled: false,
      viewMode: "own",
      regions: det.regions,
      seatRegion: [0, 1],
      mySlot: 1,
    });
    expect(crop).toBeNull();
  });
});
