/** Host PC battery as broadcast by the host (`host_battery`). */
export type HostBatteryInfo = { percent: number; charging: boolean; ac: boolean };

export type HostBatteryLevel = "ok" | "low" | "critical";

export type HostBatteryView = {
  /** Short text next to the icon, e.g. "31%". */
  label: string;
  /** 0-100, for the icon fill. */
  fill: number;
  level: HostBatteryLevel;
  /** Tooltip / screen-reader text. */
  title: string;
  /** Show the lightning bolt. */
  charging: boolean;
};

/** Windows flips power mode around 20%, and the stream hitches for a couple of seconds while it does. */
export const LOW_BATTERY_PCT = 20;
export const CRITICAL_BATTERY_PCT = 10;

/** Pure so it is unit-tested; the component only renders this. */
export function hostBatteryView(b: HostBatteryInfo): HostBatteryView {
  const pct = Math.max(0, Math.min(100, Math.round(Number.isFinite(b.percent) ? b.percent : 0)));
  const level: HostBatteryLevel =
    pct <= CRITICAL_BATTERY_PCT && !b.ac ? "critical" : pct <= LOW_BATTERY_PCT ? "low" : "ok";
  const power = b.charging ? "charging" : b.ac ? "plugged in" : "on battery";
  let title = `Host battery ${pct}% (${power})`;
  if (level !== "ok") {
    title += " — around 20% the host switches power mode and the stream can hitch for a couple of seconds";
  }
  return { label: `${pct}%`, fill: pct, level, title, charging: b.charging };
}
