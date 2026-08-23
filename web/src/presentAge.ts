/**
 * Presentation age budget for interactive streaming.
 *
 * Freshness beats completeness: never wait for an older frame in the sequence.
 * Numbers are a first cut for 60 Hz play — adaptive later.
 */

export const AGE_TARGET_MS = 25;
export const AGE_WARN_MS = 30;
export const AGE_DROP_MS = 40;
export const AGE_EMERGENCY_MS = 55;

export type AgeBand = "ok" | "warn" | "drop" | "emergency";

/** Classify receive→present age (or capture→present when available). */
export function ageBand(ageMs: number): AgeBand {
  if (ageMs > AGE_EMERGENCY_MS) return "emergency";
  if (ageMs > AGE_DROP_MS) return "drop";
  if (ageMs > AGE_WARN_MS) return "warn";
  return "ok";
}

/**
 * Should this decoded frame replace a held pending frame?
 * Always yes for newer timestamps — latest-frame-wins.
 */
export function shouldReplacePending(
  pendingTs: number | null,
  incomingTs: number
): boolean {
  if (pendingTs == null) return true;
  return incomingTs >= pendingTs;
}

/**
 * Decode-queue policy: if the decoder is already backed up, skip this AU
 * (except keyframes, which re-anchor the GOP).
 */
export function shouldSkipDecode(
  decodeQueueSize: number,
  keyframe: boolean,
  maxQueue = 1
): boolean {
  if (keyframe) return false;
  return decodeQueueSize > maxQueue;
}
