import { describe, expect, it } from "vitest";
import {
  meanPhaseStackMs,
  photonStretchMs,
  photonWowMs,
  surplusMs,
  surplusRttUnits,
  wowSurplusOk,
} from "./latencyBudget";

describe("latencyBudget", () => {
  it("ricardo wow photon is rtt plus 45", () => {
    expect(photonWowMs(48)).toBe(93);
    expect(surplusMs(93, 48)).toBe(45);
    expect(surplusRttUnits(93, 48)).toBeCloseTo(45 / 48);
  });

  it("stretch photon is rtt plus 30", () => {
    expect(photonStretchMs(48)).toBe(78);
  });

  it("mean phase stack at 250/60/60 is about 18.7ms", () => {
    expect(meanPhaseStackMs(250, 60, 60)).toBeCloseTo(18.666, 2);
  });

  it("wow surplus gate at 45ms", () => {
    expect(wowSurplusOk(44)).toBe(true);
    expect(wowSurplusOk(45)).toBe(true);
    expect(wowSurplusOk(46)).toBe(false);
  });
});
