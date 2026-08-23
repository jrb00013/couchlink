import { describe, expect, it } from "vitest";
import { ageBand, shouldReplacePending, shouldSkipDecode } from "./presentAge";

/** Canvas present-path age uses the same bands as WebCodecs. */
describe("canvas present age bands (B-R)", () => {
  it("treats sub-frame read→paint as ok", () => {
    expect(ageBand(0.5)).toBe("ok");
    expect(ageBand(10)).toBe("ok");
  });
});

describe("sacred latest-frame-wins (S6)", () => {
  it("keeps newer timestamps", () => {
    expect(shouldReplacePending(1, 2)).toBe(true);
    expect(shouldReplacePending(2, 1)).toBe(false);
  });

  it("skips deltas when decode queue backs up", () => {
    expect(shouldSkipDecode(2, false)).toBe(true);
    expect(shouldSkipDecode(2, true)).toBe(false);
  });
});
