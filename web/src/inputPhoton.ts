/**
 * Input→photon (est.): paint time minus pad send for the frame's input watermark.
 *
 * Requires CLVD v4 `input_wm` from the host. Until then, `inputFreshnessMs` is
 * a client-local lower bound only.
 */

import { surplusMs } from "./latencyBudget";

export { surplusMs } from "./latencyBudget";

let lastPadSentAt = 0;

type PadSend = { seq: number; perfSent: number; clientTsMs?: number };
const ring: PadSend[] = [];
const MAX_RING = 256;
const photonSamples: number[] = [];

export type InputPhotonSnapshot = {
  /** Last single-frame photon sample (ms). */
  lastPhotonMs: number | null;
  photonP50Ms: number | null;
  surplusP50Ms: number | null;
  sampleCount: number;
  ringSize: number;
  inputFreshnessMs: number | null;
  /** True once CLVD input_wm samples have landed. */
  watermarkActive: boolean;
};

export function notePadSent(
  atMs = performance.now(),
  seq?: number,
  clientTsMs?: number
): void {
  lastPadSentAt = atMs;
  if (seq != null) {
    ring.push({
      seq: seq >>> 0,
      perfSent: atMs,
      clientTsMs: clientTsMs !== undefined ? clientTsMs >>> 0 : undefined,
    });
    if (ring.length > MAX_RING) ring.shift();
  }
}

export function resetInputPhoton(): void {
  lastPadSentAt = 0;
  ring.length = 0;
  photonSamples.length = 0;
}

/** Ms since last pad send at paint. null if no pad sent yet. */
export function inputFreshnessMs(paintMs = performance.now()): number | null {
  if (lastPadSentAt <= 0) return null;
  return Math.max(0, paintMs - lastPadSentAt);
}

/** Record one photon sample when a frame paints with a pad watermark. */
export function notePhotonPaint(paintMs: number, inputWm: number): number | null {
  if (!inputWm) return null;
  const wm = inputWm >>> 0;
  const hit = [...ring].reverse().find((e) => e.seq === wm);
  if (!hit) return null;
  // Always use full-float perfSent. clientTsMs is u32-truncated for the wire and
  // must not be the Φ clock (truncation + wrong clock ⇒ bogus 100–300ms S).
  const ms = Math.max(0, paintMs - hit.perfSent);
  photonSamples.push(ms);
  if (photonSamples.length > 120) photonSamples.shift();
  return ms;
}

function percentile(sorted: number[], p: number): number | null {
  if (sorted.length === 0) return null;
  const idx = Math.floor((sorted.length - 1) * p);
  return sorted[idx] ?? null;
}

export function photonP50Ms(): number | null {
  if (photonSamples.length === 0) return null;
  const s = [...photonSamples].sort((a, b) => a - b);
  return percentile(s, 0.5);
}

export function surplusP50Ms(rttMs: number): number | null {
  const p50 = photonP50Ms();
  if (p50 == null || !Number.isFinite(rttMs)) return null;
  return surplusMs(p50, rttMs);
}

export function lastPhotonMs(): number | null {
  if (photonSamples.length === 0) return null;
  return photonSamples[photonSamples.length - 1] ?? null;
}

/** Full local input→photon snapshot for the debug Latency tab. */
export function getInputPhotonSnapshot(rttMs: number): InputPhotonSnapshot {
  const p50 = photonP50Ms();
  return {
    lastPhotonMs: lastPhotonMs(),
    photonP50Ms: p50,
    surplusP50Ms: surplusP50Ms(rttMs),
    sampleCount: photonSamples.length,
    ringSize: ring.length,
    inputFreshnessMs: inputFreshnessMs(),
    watermarkActive: photonSamples.length > 0,
  };
}
