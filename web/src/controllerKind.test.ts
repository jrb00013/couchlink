import { describe, expect, it } from "vitest";
import {
  controllerKind,
  isViGEmXbox360Cluster,
  selectPhysicalGamepads,
} from "./controllerKind";

describe("controllerKind", () => {
  it("detects Xbox Series / One", () => {
    expect(
      controllerKind(
        "Xbox Series X Controller (STANDARD GAMEPAD Vendor: 045e Product: 0b12)"
      )
    ).toBe("xbox");
    expect(
      controllerKind("Xbox Wireless Controller (Vendor: 045e Product: 02fd)")
    ).toBe("xbox");
  });

  it("detects DualSense / DualShock", () => {
    expect(
      controllerKind(
        "DualSense Wireless Controller (STANDARD GAMEPAD Vendor: 054c Product: 0ce6)"
      )
    ).toBe("dualsense");
    expect(
      controllerKind(
        "Wireless Controller (STANDARD GAMEPAD Vendor: 054c Product: 09cc)"
      )
    ).toBe("dualsense");
  });

  it("falls back to generic", () => {
    expect(controllerKind("Generic USB Joystick")).toBe("generic");
  });

  it("treats two identical Xbox 360 ids as the host ViGEm cluster", () => {
    const id = "Xbox 360 Controller (XInput STANDARD GAMEPAD)";
    expect(isViGEmXbox360Cluster([id, id, id, id])).toBe(true);
    expect(
      selectPhysicalGamepads([
        { id },
        { id },
        { id },
        { id },
        { id: "DualSense Wireless Controller (STANDARD GAMEPAD Vendor: 054c Product: 0ce6)" },
      ]).map((p) => p.id)
    ).toEqual([
      "DualSense Wireless Controller (STANDARD GAMEPAD Vendor: 054c Product: 0ce6)",
    ]);
  });

  it("keeps a single real Xbox 360 in a friend's browser", () => {
    const id = "Xbox 360 Controller (XInput STANDARD GAMEPAD)";
    expect(isViGEmXbox360Cluster([id])).toBe(false);
    expect(selectPhysicalGamepads([{ id }])).toEqual([{ id }]);
  });
});
