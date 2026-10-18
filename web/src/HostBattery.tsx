import { hostBatteryView, type HostBatteryInfo } from "./hostBattery";

/** Small battery icon + percent for the HOST's battery, so friends can tell a power-mode hitch from a network problem. */
export function HostBattery({ battery }: { battery: HostBatteryInfo }) {
  const v = hostBatteryView(battery);
  return (
    <span className={`host-battery is-${v.level}`} title={v.title} aria-label={v.title} role="status">
      <span className="host-battery-icon" aria-hidden="true">
        <span className="host-battery-fill" style={{ width: `${v.fill}%` }} />
      </span>
      <span className="host-battery-label">
        {v.charging ? "⚡" : ""}
        {v.label}
      </span>
    </span>
  );
}
