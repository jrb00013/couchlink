import { describe, expect, it } from "vitest";
import { hostBatteryView } from "./hostBattery";

describe("hostBatteryView", () => {
  it("healthy plugged-in battery is ok and says charging", () => {
    const v = hostBatteryView({ percent: 31, charging: true, ac: true });
    expect(v.level).toBe("ok");
    expect(v.label).toBe("31%");
    expect(v.charging).toBe(true);
    expect(v.title).toContain("charging");
  });

  it("20% is low even while charging (the power-mode switch happens there)", () => {
    const v = hostBatteryView({ percent: 20, charging: true, ac: true });
    expect(v.level).toBe("low");
    expect(v.title).toContain("hitch");
  });

  it("21% is still ok", () => {
    expect(hostBatteryView({ percent: 21, charging: false, ac: false }).level).toBe("ok");
  });

  it("10% on battery is critical, but plugged in at 10% is only low", () => {
    expect(hostBatteryView({ percent: 10, charging: false, ac: false }).level).toBe("critical");
    expect(hostBatteryView({ percent: 10, charging: true, ac: true }).level).toBe("low");
  });

  it("plugged in at 100% says plugged in, not charging", () => {
    const v = hostBatteryView({ percent: 100, charging: false, ac: true });
    expect(v.title).toContain("plugged in");
    expect(v.charging).toBe(false);
    expect(v.level).toBe("ok");
  });

  it("clamps garbage", () => {
    expect(hostBatteryView({ percent: 250, charging: false, ac: false }).fill).toBe(100);
    expect(hostBatteryView({ percent: -5, charging: false, ac: false }).fill).toBe(0);
    expect(hostBatteryView({ percent: NaN, charging: false, ac: false }).fill).toBe(0);
  });
});
