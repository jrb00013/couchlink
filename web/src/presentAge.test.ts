import { describe, expect, it } from "vitest";
import {
  ageBand,
  AGE_DROP_MS,
  AGE_EMERGENCY_MS,
  AGE_TARGET_MS,
  AGE_WARN_MS,
  shouldReplacePending,
  shouldSkipDecode,
} from "./presentAge";

describe("ageBand", () => {
  it("classifies within the interactive budget", () => {
    expect(ageBand(0)).toBe("ok");
    expect(ageBand(AGE_TARGET_MS)).toBe("ok");
    expect(ageBand(AGE_WARN_MS + 0.1)).toBe("warn");
    expect(ageBand(AGE_DROP_MS + 0.1)).toBe("drop");
    expect(ageBand(AGE_EMERGENCY_MS + 0.1)).toBe("emergency");
  });
});

describe("shouldReplacePending", () => {
  it("always accepts the first frame", () => {
    expect(shouldReplacePending(null, 1)).toBe(true);
  });

  it("keeps the newer timestamp (latest-frame-wins)", () => {
    expect(shouldReplacePending(100, 200)).toBe(true);
    expect(shouldReplacePending(200, 200)).toBe(true);
    expect(shouldReplacePending(200, 100)).toBe(false);
  });
});

describe("shouldSkipDecode", () => {
  it("never skips keyframes", () => {
    expect(shouldSkipDecode(99, true)).toBe(false);
  });

  it("skips deltas when the decoder queue is backed up", () => {
    expect(shouldSkipDecode(0, false)).toBe(false);
    expect(shouldSkipDecode(1, false)).toBe(false);
    expect(shouldSkipDecode(2, false)).toBe(true);
  });
});
