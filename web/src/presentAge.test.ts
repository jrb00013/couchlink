import { describe, expect, it } from "vitest";
import {
  ageBand,
  AGE_DROP_MS,
  AGE_EMERGENCY_MS,
  AGE_TARGET_MS,
  AGE_WARN_MS,
  decodeBacklogPolicy,
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
    expect(shouldSkipDecode(3, false)).toBe(false);
    expect(shouldSkipDecode(4, false)).toBe(true);
  });

  it("maxQueue 3 leaves headroom for ~70fps paint at high push", () => {
    expect(shouldSkipDecode(2, false)).toBe(false);
    expect(shouldSkipDecode(3, false)).toBe(false);
  });
});

describe("decodeBacklogPolicy", () => {
  it("skips deltas on backlog without requesting IDR (CPU congestion)", () => {
    expect(decodeBacklogPolicy(4, false)).toBe("skip");
    expect(decodeBacklogPolicy(4, false, "ok")).toBe("skip");
    expect(decodeBacklogPolicy(4, false, "warn")).toBe("skip");
  });

  it("requests IDR only when backlog coincides with late age", () => {
    expect(decodeBacklogPolicy(4, false, "drop")).toBe("skip-request-idr");
    expect(decodeBacklogPolicy(4, false, "emergency")).toBe("skip-request-idr");
  });

  it("decodes when queue is healthy", () => {
    expect(decodeBacklogPolicy(1, false)).toBe("decode");
  });
});
