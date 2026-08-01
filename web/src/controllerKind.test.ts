import { describe, expect, it } from "vitest";
import { controllerKind } from "./controllerKind";

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
});
