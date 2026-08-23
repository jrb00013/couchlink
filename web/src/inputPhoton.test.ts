import { describe, expect, it, beforeEach } from "vitest";
import {
  inputFreshnessMs,
  notePadSent,
  resetInputPhoton,
} from "./inputPhoton";

describe("inputPhoton", () => {
  beforeEach(() => resetInputPhoton());

  it("returns null before any pad send", () => {
    expect(inputFreshnessMs(100)).toBeNull();
  });

  it("reports freshness as paint − last pad send", () => {
    notePadSent(10);
    expect(inputFreshnessMs(25)).toBe(15);
  });

  it("never goes negative", () => {
    notePadSent(100);
    expect(inputFreshnessMs(50)).toBe(0);
  });
});
