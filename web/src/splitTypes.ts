import type { NormRect } from "./proto";

export type SplitLayout = "none" | "horizontal" | "vertical" | "grid2x2";

export type SplitDetection = {
  layout: SplitLayout;
  regions: NormRect[];
  score: number;
};

export type ViewMode = "own" | "full";

export type SplitSeatConfig = {
  masterEnabled: boolean;
  hostViewMode: ViewMode;
  layout: SplitLayout;
  regions: NormRect[];
  seatRegion: number[];
  rescan: boolean;
};

export const FULL_FRAME: NormRect = { x: 0, y: 0, w: 1, h: 1 };

export function clampRect(r: NormRect): NormRect {
  const x = Math.min(1, Math.max(0, r.x));
  const y = Math.min(1, Math.max(0, r.y));
  const w = Math.min(1 - x, Math.max(0, r.w));
  const h = Math.min(1 - y, Math.max(0, r.h));
  return { x, y, w, h };
}
