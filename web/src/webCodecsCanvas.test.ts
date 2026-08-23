import { describe, expect, it } from "vitest";
import {
  pickHardwareAcceleration,
  webcodecsDiagnosis,
} from "./webCodecsCanvas";

describe("webcodecsDiagnosis", () => {
  it("labels a fast, full-rate stream healthy", () => {
    expect(webcodecsDiagnosis(60, 2.1, 0)).toBe("healthy");
  });

  it("labels slow decode as decode-bound regardless of frame rate", () => {
    // Decode eating more than half a 16.7ms frame budget is a real local
    // cost, not noise — this must win even if fps still looks fine.
    expect(webcodecsDiagnosis(58, 12.4, 0)).toContain("decode-bound");
  });

  it("labels dropped frames plus low fps as likely network loss", () => {
    const d = webcodecsDiagnosis(30, 2.0, 5);
    expect(d).toContain("network loss");
    expect(d).toContain("COUCHLINK_FEC");
  });

  it("labels low fps with fast decode and no drops as upstream, not local", () => {
    const d = webcodecsDiagnosis(20, 1.5, 0);
    expect(d).toContain("host or network side");
    expect(d).not.toContain("decode-bound");
  });

  it("decode-bound takes priority over the drop-based labels", () => {
    // If decode itself is slow, that's the finding — don't bury it behind a
    // network-loss guess just because frames were also dropped.
    const d = webcodecsDiagnosis(20, 15, 5);
    expect(d).toContain("decode-bound");
  });
});

describe("pickHardwareAcceleration", () => {
  it("prefers hardware when the GPU path is supported", () => {
    expect(
      pickHardwareAcceleration([
        { accel: "prefer-hardware", supported: true },
        { accel: "prefer-software", supported: true },
        { accel: "no-preference", supported: true },
      ])
    ).toBe("prefer-hardware");
  });

  it("falls back to software when hardware is unsupported (headless/WSL)", () => {
    expect(
      pickHardwareAcceleration([
        { accel: "prefer-hardware", supported: false },
        { accel: "prefer-software", supported: true },
        { accel: "no-preference", supported: true },
      ])
    ).toBe("prefer-software");
  });
});
