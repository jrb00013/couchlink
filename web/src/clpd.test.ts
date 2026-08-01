import { describe, expect, it } from "vitest";
import {
  BTN,
  encodeClpd,
  fromBrowserGamepad,
  type PadState,
} from "./clpd";

/** Minimal Gamepad stand-in for Standard mapping (Xbox / DualSense / DualShock 4). */
function fakeGamepad(opts: {
  id: string;
  pressed?: number[];
  values?: Record<number, number>;
  axes?: number[];
}): Gamepad {
  const pressed = new Set(opts.pressed ?? []);
  const values = opts.values ?? {};
  const buttons: GamepadButton[] = Array.from({ length: 18 }, (_, i) => {
    const value = values[i] ?? (pressed.has(i) ? 1 : 0);
    return {
      pressed: pressed.has(i) || value > 0.1,
      touched: pressed.has(i) || value > 0.1,
      value,
    };
  });
  return {
    id: opts.id,
    index: 0,
    connected: true,
    mapping: "standard",
    axes: opts.axes ?? [0, 0, 0, 0],
    buttons,
    timestamp: 0,
    hapticActuators: [],
    vibrationActuator: null,
  } as unknown as Gamepad;
}

describe("controller tester — browser Gamepad → CLPD", () => {
  it("recognizes Xbox Series-style id and maps A→CROSS", () => {
    const gp = fakeGamepad({
      id: "Xbox Series X Controller (STANDARD GAMEPAD Vendor: 045e Product: 0b12)",
      pressed: [0], // A
    });
    expect(gp.id.toLowerCase()).toContain("xbox");
    const state = fromBrowserGamepad(gp, 1);
    expect(state.buttons & BTN.CROSS).toBeTruthy();
    expect(state.buttons & BTN.CIRCLE).toBe(0);
  });

  it("maps full Xbox face / shoulder / dpad matrix", () => {
    const cases: Array<[number, number]> = [
      [0, BTN.CROSS],
      [1, BTN.CIRCLE],
      [2, BTN.SQUARE],
      [3, BTN.TRIANGLE],
      [4, BTN.L1],
      [5, BTN.R1],
      [8, BTN.CREATE],
      [9, BTN.OPTIONS],
      [10, BTN.L3],
      [11, BTN.R3],
      [12, BTN.DPAD_UP],
      [13, BTN.DPAD_DOWN],
      [14, BTN.DPAD_LEFT],
      [15, BTN.DPAD_RIGHT],
      [16, BTN.PS],
    ];
    for (const [index, bit] of cases) {
      const state = fromBrowserGamepad(
        fakeGamepad({
          id: "Xbox Wireless Controller (Vendor: 045e Product: 02fd)",
          pressed: [index],
        }),
        0,
      );
      expect(state.buttons & bit).toBeTruthy();
    }
  });

  it("maps DualSense (PS5) Cross and touchpad", () => {
    const gp = fakeGamepad({
      id: "DualSense Wireless Controller (STANDARD GAMEPAD Vendor: 054c Product: 0ce6)",
      pressed: [0, 17],
    });
    expect(gp.id.toLowerCase()).toContain("dualsense");
    const state = fromBrowserGamepad(gp, 2);
    expect(state.buttons & BTN.CROSS).toBeTruthy();
    expect(state.buttons & BTN.TOUCH).toBeTruthy();
  });

  it("maps DualShock 4 (PS4) the same Standard way", () => {
    // Browser path: any Standard Gamepad — including PS4 DualShock 4.
    const gp = fakeGamepad({
      id: "Wireless Controller (STANDARD GAMEPAD Vendor: 054c Product: 09cc)",
      pressed: [0, 1, 2, 3],
    });
    expect(gp.id).toContain("054c");
    expect(gp.id).toContain("09cc");
    const state = fromBrowserGamepad(gp, 3);
    expect(state.buttons & BTN.CROSS).toBeTruthy();
    expect(state.buttons & BTN.CIRCLE).toBeTruthy();
    expect(state.buttons & BTN.SQUARE).toBeTruthy();
    expect(state.buttons & BTN.TRIANGLE).toBeTruthy();
  });

  it("maps DualShock 4 v1 product 05c4", () => {
    const gp = fakeGamepad({
      id: "Wireless Controller (STANDARD GAMEPAD Vendor: 054c Product: 05c4)",
      pressed: [16],
    });
    const state = fromBrowserGamepad(gp, 4);
    expect(state.buttons & BTN.PS).toBeTruthy();
  });

  it("encodes triggers and sticks into CLPD for host", () => {
    const gp = fakeGamepad({
      id: "Xbox One S Controller (Vendor: 045e Product: 02e0)",
      values: { 6: 1, 7: 0.5 },
      axes: [1, -1, 0.5, -0.5],
    });
    const state: PadState = fromBrowserGamepad(gp, 9);
    expect(state.buttons & BTN.L2).toBeTruthy();
    expect(state.buttons & BTN.R2).toBeTruthy();
    expect(state.l2).toBe(255);
    expect(state.r2).toBeGreaterThan(100);
    expect(state.lx).toBe(255);
    expect(state.ly).toBe(0);

    const buf = new Uint8Array(encodeClpd(state));
    expect(buf[0]).toBe(0x43); // C
    expect(buf[1]).toBe(0x4c); // L
    expect(buf[2]).toBe(0x50); // P
    expect(buf[3]).toBe(0x44); // D
    expect(buf.length).toBe(31);
    // seq little-endian at offset 5
    expect(buf[5]).toBe(9);
  });

  it("neutral pad encodes zero buttons", () => {
    const state = fromBrowserGamepad(
      fakeGamepad({
        id: "DualSense Edge Wireless Controller (Vendor: 054c Product: 0df2)",
      }),
      0,
    );
    expect(state.buttons).toBe(0);
    expect(state.lx).toBe(128);
    expect(state.ly).toBe(128);
  });
});
