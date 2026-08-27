/** Why present stayed off WebCodecs — structured stuck taxonomy (amazing T1). */

export type PresentStuckReason =
  | "no_au"
  | "decoder_fail"
  | "fallback_timer"
  | "ua_legacy"
  | "stall_warmup";

export function classifyPresentStuck(opts: {
  preferLegacy: boolean;
  hasDecoder: boolean;
  sawAu: boolean;
  painted: boolean;
  stalled: boolean;
  fallbackFired: boolean;
}): PresentStuckReason | null {
  if (opts.preferLegacy) return "ua_legacy";
  if (!opts.hasDecoder) return "decoder_fail";
  if (opts.stalled) return "stall_warmup";
  if (opts.fallbackFired && !opts.painted) return "fallback_timer";
  if (!opts.sawAu) return "no_au";
  return null;
}
