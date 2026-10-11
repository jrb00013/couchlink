/**
 * Non-regression: isolation OFF must be the legacy full-frame paint path.
 * Isolation ON must never require a second encode — crop is draw-time only.
 */
import { describe, expect, it } from "vitest";
import { cropForClient, defaultRegions, parseSplitConfig } from "./splitSeat";
import { FULL_FRAME } from "./splitTypes";

describe("kaleidoscope non-regression (master off = today's paint)", () => {
  const regions = defaultRegions("vertical");

  it("master disabled → null crop (callers use drawImage(frame,0,0))", () => {
    expect(
      cropForClient({
        masterEnabled: false,
        viewMode: "own",
        regions,
        seatRegion: [0, 1],
        mySlot: 1,
      }),
    ).toBeNull();
  });

  it("master on + full view → null crop (same full-frame path)", () => {
    expect(
      cropForClient({
        masterEnabled: true,
        viewMode: "full",
        regions,
        seatRegion: [0, 1],
        mySlot: 1,
      }),
    ).toBeNull();
  });

  it("default parse keeps master off (session join does not arm isolation)", () => {
    const c = parseSplitConfig({ master_enabled: false });
    expect(c.masterEnabled).toBe(false);
    expect(
      cropForClient({
        masterEnabled: c.masterEnabled,
        viewMode: "own",
        regions: c.regions,
        seatRegion: c.seatRegion,
        mySlot: 1,
      }),
    ).toBeNull();
  });

  it("own crop is a pure NormRect subset — no upscale/encode metadata", () => {
    const r = cropForClient({
      masterEnabled: true,
      viewMode: "own",
      regions,
      seatRegion: [0, 1],
      mySlot: 1,
    });
    expect(r).not.toBeNull();
    expect(r!.w).toBeLessThanOrEqual(1);
    expect(r!.h).toBeLessThanOrEqual(1);
    // Not the full frame when vertical split + own seat for remote P2.
    expect(r).not.toEqual(FULL_FRAME);
  });
});
