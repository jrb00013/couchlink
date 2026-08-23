/**
 * Client-local input freshness at paint time.
 *
 * Not true input→photon (needs host frame watermarks). This is "how long since
 * we last sent a pad state when this frame painted" — a lower bound / sanity
 * metric until CLVD carries an input watermark.
 */

let lastPadSentAt = 0;

export function notePadSent(atMs = performance.now()): void {
  lastPadSentAt = atMs;
}

export function resetInputPhoton(): void {
  lastPadSentAt = 0;
}

/** Ms since last pad send at paint. null if no pad sent yet. */
export function inputFreshnessMs(paintMs = performance.now()): number | null {
  if (lastPadSentAt <= 0) return null;
  return Math.max(0, paintMs - lastPadSentAt);
}
