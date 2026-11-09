import type { NormRect } from "./proto";
import type { SplitLayout, SplitSeatConfig, ViewMode } from "./splitTypes";
import { FULL_FRAME, clampRect } from "./splitTypes";
import { seatForRemoteSlot } from "./seat";

/** Parse host `split_screen_config` into a typed config. */
export function parseSplitConfig(msg: {
  master_enabled: boolean;
  host_view_mode?: string;
  layout?: string;
  regions?: NormRect[];
  seat_region?: number[];
  rescan?: boolean;
}): SplitSeatConfig {
  const layout = (msg.layout ?? "none") as SplitLayout;
  const hostViewMode = (msg.host_view_mode === "own" ? "own" : "full") as ViewMode;
  return {
    masterEnabled: !!msg.master_enabled,
    hostViewMode,
    layout: ["none", "horizontal", "vertical", "grid2x2"].includes(layout)
      ? layout
      : "none",
    regions: (msg.regions ?? []).map(clampRect),
    seatRegion: (msg.seat_region ?? [0, 1, 2, 3]).map((n) => Number(n) | 0),
    rescan: !!msg.rescan,
  };
}

/**
 * Pick the paint crop for this client.
 * Host is seat 1 / region index via seatRegion[0]. Remotes use seatForRemoteSlot.
 * When master off or viewMode full → full frame (byte-identical to today).
 */
export function cropForClient(opts: {
  masterEnabled: boolean;
  viewMode: ViewMode;
  regions: NormRect[];
  seatRegion: number[];
  /** null = host (local P1) */
  mySlot: number | null;
  isHost?: boolean;
}): NormRect | null {
  if (!opts.masterEnabled || opts.viewMode === "full") {
    return null; // callers treat null as full-frame drawImage
  }
  const regions = opts.regions;
  if (!regions.length) return null;

  let seat: number;
  if (opts.isHost || opts.mySlot == null) {
    seat = 1;
  } else {
    seat = seatForRemoteSlot(opts.mySlot);
  }
  const seatIdx = Math.max(0, Math.min(3, seat - 1));
  const regionIdx = opts.seatRegion[seatIdx] ?? seatIdx;
  const rect = regions[regionIdx] ?? regions[0];
  return rect ? clampRect(rect) : FULL_FRAME;
}

export function defaultRegions(layout: SplitLayout): NormRect[] {
  switch (layout) {
    case "horizontal":
      return [
        { x: 0, y: 0, w: 1, h: 0.5 },
        { x: 0, y: 0.5, w: 1, h: 0.5 },
      ];
    case "vertical":
      return [
        { x: 0, y: 0, w: 0.5, h: 1 },
        { x: 0.5, y: 0, w: 0.5, h: 1 },
      ];
    case "grid2x2":
      return [
        { x: 0, y: 0, w: 0.5, h: 0.5 },
        { x: 0.5, y: 0, w: 0.5, h: 0.5 },
        { x: 0, y: 0.5, w: 0.5, h: 0.5 },
        { x: 0.5, y: 0.5, w: 0.5, h: 0.5 },
      ];
    default:
      return [FULL_FRAME];
  }
}
