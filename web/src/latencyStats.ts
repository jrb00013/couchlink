/**
 * Pure helpers for browser latency telemetry.
 * Thresholds lock the measured LAN baseline from the GPU-encode session:
 *   jitterBufferMs ≈ 6–9, decodeFps ≈ 59, framesDropped = 0
 */

export type InboundVideoStatsSample = {
  jitterBufferDelay: number;
  jitterBufferEmittedCount: number;
  framesDecoded: number;
  framesDropped: number;
};

export type JitterWindow = {
  jitterBufferMs: number;
  decodeFps: number;
  framesDropped: number;
};

/** Delta over a getStats window (same math as CouchlinkPlayer.startStatsPolling). */
export function jitterWindow(
  prev: InboundVideoStatsSample,
  next: InboundVideoStatsSample,
  elapsedSec: number
): JitterWindow | null {
  const countDelta = next.jitterBufferEmittedCount - prev.jitterBufferEmittedCount;
  if (countDelta <= 0 || elapsedSec <= 0) return null;
  const delayDelta = next.jitterBufferDelay - prev.jitterBufferDelay;
  const decodedDelta = next.framesDecoded - prev.framesDecoded;
  return {
    jitterBufferMs: (delayDelta / countDelta) * 1000,
    decodeFps: decodedDelta / elapsedSec,
    framesDropped: next.framesDropped,
  };
}

/** LAN co-play regression gates — fail the build if we regress past these. */
export const LAN_LATENCY_GATES = {
  /** Chrome floor we measured after canvas + JB pin was ~6–9ms. */
  maxJitterBufferMs: 20,
  /** 720p60 path should hold near full rate on LAN. */
  minDecodeFps: 50,
  /** Drops mean the present path or decoder fell behind. */
  maxFramesDropped: 0,
} as const;

export type GateResult = {
  ok: boolean;
  failures: string[];
  sample: JitterWindow;
};

export function evaluateLanLatency(sample: JitterWindow): GateResult {
  const failures: string[] = [];
  if (sample.jitterBufferMs > LAN_LATENCY_GATES.maxJitterBufferMs) {
    failures.push(
      `jitterBufferMs ${sample.jitterBufferMs.toFixed(1)} > ${LAN_LATENCY_GATES.maxJitterBufferMs}`
    );
  }
  if (sample.decodeFps < LAN_LATENCY_GATES.minDecodeFps) {
    failures.push(
      `decodeFps ${sample.decodeFps.toFixed(1)} < ${LAN_LATENCY_GATES.minDecodeFps}`
    );
  }
  if (sample.framesDropped > LAN_LATENCY_GATES.maxFramesDropped) {
    failures.push(
      `framesDropped ${sample.framesDropped} > ${LAN_LATENCY_GATES.maxFramesDropped}`
    );
  }
  return { ok: failures.length === 0, failures, sample };
}
