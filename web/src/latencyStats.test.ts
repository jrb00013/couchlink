import { describe, expect, it } from "vitest";
import {
  evaluateLanLatency,
  jitterWindow,
  LAN_LATENCY_GATES,
} from "./latencyStats";

describe("jitterWindow", () => {
  it("matches the measured LAN baseline (~7ms JB, ~59fps)", () => {
    // Synthetic window mirroring the console samples from the GPU path session.
    const prev = {
      jitterBufferDelay: 1.0,
      jitterBufferEmittedCount: 100,
      framesDecoded: 1000,
      framesDropped: 0,
    };
    const next = {
      jitterBufferDelay: 1.0 + (7 / 1000) * 120, // 7ms average over 120 frames
      jitterBufferEmittedCount: 220,
      framesDecoded: 1000 + 118, // ~59 fps over 2s
      framesDropped: 0,
    };
    const w = jitterWindow(prev, next, 2);
    expect(w).not.toBeNull();
    expect(w!.jitterBufferMs).toBeCloseTo(7, 0);
    expect(w!.decodeFps).toBeCloseTo(59, 0);
    expect(evaluateLanLatency(w!).ok).toBe(true);
  });

  it("fails the gate when the jitter buffer regresses above 20ms", () => {
    const prev = {
      jitterBufferDelay: 0,
      jitterBufferEmittedCount: 0,
      framesDecoded: 0,
      framesDropped: 0,
    };
    const next = {
      jitterBufferDelay: (50 / 1000) * 60,
      jitterBufferEmittedCount: 60,
      framesDecoded: 60,
      framesDropped: 0,
    };
    const w = jitterWindow(prev, next, 1)!;
    expect(w.jitterBufferMs).toBeCloseTo(50, 0);
    const gate = evaluateLanLatency(w);
    expect(gate.ok).toBe(false);
    expect(gate.failures.some((f) => f.includes("jitterBufferMs"))).toBe(true);
  });

  it("exposes stable LAN gates for the regression harness", () => {
    expect(LAN_LATENCY_GATES.maxJitterBufferMs).toBe(20);
    expect(LAN_LATENCY_GATES.minDecodeFps).toBe(50);
    expect(LAN_LATENCY_GATES.maxFramesDropped).toBe(0);
  });
});
