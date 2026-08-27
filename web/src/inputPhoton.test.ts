import { describe, expect, it, beforeEach } from "vitest";
import {
  getInputPhotonSnapshot,
  inputFreshnessMs,
  notePadSent,
  notePhotonPaint,
  photonP50Ms,
  resetInputPhoton,
  surplusMs,
  surplusP50Ms,
} from "./inputPhoton";

describe("inputPhoton", () => {
  beforeEach(() => resetInputPhoton());

  it("freshness improves when pad sends on the same tick as paint", () => {
    notePadSent(100);
    notePhotonPaint(102, 1);
    expect(inputFreshnessMs(102)).toBe(2);
  });

  it("reports freshness as paint − last pad send", () => {
    notePadSent(10);
    expect(inputFreshnessMs(25)).toBe(15);
  });

  it("never goes negative", () => {
    notePadSent(100);
    expect(inputFreshnessMs(50)).toBe(0);
  });

  it("computes photon from input watermark seq", () => {
    notePadSent(100, 5);
    expect(notePhotonPaint(190, 5)).toBe(90);
    expect(photonP50Ms()).toBe(90);
  });

  it("uses perfSent not truncated clientTsMs for Φ", () => {
    notePadSent(100, 5, 50); // wire ts deliberately wrong/truncated
    expect(notePhotonPaint(190, 5)).toBe(90);
  });

  it("surplus subtracts RTT from photon p50", () => {
    notePadSent(100, 5);
    notePhotonPaint(190, 5);
    expect(surplusMs(90, 48)).toBe(42);
    expect(surplusP50Ms(48)).toBe(42);
  });

  it("ignores paint when watermark seq not in ring", () => {
    notePadSent(100, 5);
    expect(notePhotonPaint(200, 99)).toBeNull();
    expect(photonP50Ms()).toBeNull();
  });

  it("snapshot bundles ring state for the debug drawer", () => {
    notePadSent(100, 5);
    notePhotonPaint(190, 5);
    const snap = getInputPhotonSnapshot(48);
    expect(snap.lastPhotonMs).toBe(90);
    expect(snap.photonP50Ms).toBe(90);
    expect(snap.surplusP50Ms).toBe(42);
    expect(snap.watermarkActive).toBe(true);
    expect(snap.sampleCount).toBe(1);
  });
});
