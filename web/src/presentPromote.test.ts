import { describe, expect, it } from "vitest";
import { classifyPresentStuck } from "./presentPromote";

describe("classifyPresentStuck", () => {
  const ok = {
    preferLegacy: false,
    hasDecoder: true,
    sawAu: true,
    painted: true,
    stalled: false,
    fallbackFired: false,
  };

  it("returns null when painted and healthy", () => {
    expect(classifyPresentStuck(ok)).toBeNull();
  });

  it("ua_legacy", () => {
    expect(classifyPresentStuck({ ...ok, preferLegacy: true })).toBe("ua_legacy");
  });

  it("decoder_fail", () => {
    expect(classifyPresentStuck({ ...ok, hasDecoder: false, painted: false })).toBe(
      "decoder_fail"
    );
  });

  it("stall_warmup", () => {
    expect(classifyPresentStuck({ ...ok, stalled: true, painted: false })).toBe(
      "stall_warmup"
    );
  });

  it("fallback_timer", () => {
    expect(
      classifyPresentStuck({
        ...ok,
        painted: false,
        fallbackFired: true,
      })
    ).toBe("fallback_timer");
  });

  it("no_au", () => {
    expect(classifyPresentStuck({ ...ok, sawAu: false, painted: false })).toBe("no_au");
  });
});
