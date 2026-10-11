import { describe, expect, it } from "vitest";
import { cropForClient, defaultRegions, parseSplitConfig } from "./splitSeat";

describe("parseSplitConfig", () => {
  it("defaults safely", () => {
    const c = parseSplitConfig({ master_enabled: false });
    expect(c.masterEnabled).toBe(false);
    expect(c.hostViewMode).toBe("full");
    expect(c.layout).toBe("none");
  });
});

describe("cropForClient", () => {
  const regions = defaultRegions("vertical");

  it("returns null when master off (full-frame path)", () => {
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

  it("returns null for full view mode", () => {
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

  it("maps remote slot 1 → seat P2 → second vertical region", () => {
    const r = cropForClient({
      masterEnabled: true,
      viewMode: "own",
      regions,
      seatRegion: [0, 1],
      mySlot: 1,
    });
    expect(r).toEqual({ x: 0.5, y: 0, w: 0.5, h: 1 });
  });

  it("host uses seat 1 / first region", () => {
    const r = cropForClient({
      masterEnabled: true,
      viewMode: "own",
      regions,
      seatRegion: [0, 1],
      mySlot: null,
      isHost: true,
    });
    expect(r).toEqual({ x: 0, y: 0, w: 0.5, h: 1 });
  });
});
