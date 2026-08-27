/** Mirrors `crates/host/src/input_photon_budget.rs` — cited constants only. */

export const WOW_SURPLUS_MS = 45;
export const STRETCH_SURPLUS_MS = 30;
export const SHM_WAIT_P95_GATE_MS = 1.0;

export function surplusMs(phiMs: number, rttMs: number): number {
  return phiMs - rttMs;
}

export function surplusRttUnits(phiMs: number, rttMs: number): number {
  if (rttMs <= 0) return 0;
  return surplusMs(phiMs, rttMs) / rttMs;
}

export function photonWowMs(rttMs: number): number {
  return rttMs + WOW_SURPLUS_MS;
}

export function photonStretchMs(rttMs: number): number {
  return rttMs + STRETCH_SURPLUS_MS;
}

export function wowSurplusOk(surplusP50Ms: number): boolean {
  return surplusP50Ms <= WOW_SURPLUS_MS;
}

export function meanPhaseWaitMs(hz: number): number {
  if (hz <= 0) return 0;
  return 1000 / hz / 2;
}

export function meanPhaseStackMs(
  padHz: number,
  videoFps: number,
  displayFps: number
): number {
  return (
    meanPhaseWaitMs(padHz) +
    meanPhaseWaitMs(videoFps) +
    meanPhaseWaitMs(displayFps)
  );
}

export function handoffWaitPeriods(waitMs: number, videoFps: number): number {
  if (videoFps <= 0) return 0;
  return waitMs / (1000 / videoFps);
}

export function shmGateTrips(waitP95Ms: number): boolean {
  return waitP95Ms > SHM_WAIT_P95_GATE_MS;
}
