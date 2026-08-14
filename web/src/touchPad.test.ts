import { describe, expect, it } from "vitest";
import { BTN, encodeClpd, type PadState } from "./clpd";
import { sampleTouch, TOUCH_RADIUS, type TouchButton } from "./touchPad";

describe("touchPad — touch-screen → CLPD", () => {
  it("neutral touch emits centered sticks and zero buttons", () => {
    const state = sampleTouch(1, { active: false, dx: 0, dy: 0 }, { active: false, dx: 0, dy: 0 }, new Set());
    expect(state.buttons).toBe(0);
    expect(state.lx).toBe(128);
    expect(state.ly).toBe(128);
    expect(state.rx).toBe(128);
    expect(state.ry).toBe(128);
    expect(state.l2).toBe(0);
    expect(state.r2).toBe(0);
  });

  it("maps every touch button to the DualSense bit", () => {
    const cases: Array<[TouchButton, number]> = [
      ["cross", BTN.CROSS],
      ["circle", BTN.CIRCLE],
      ["square", BTN.SQUARE],
      ["triangle", BTN.TRIANGLE],
      ["l1", BTN.L1],
      ["r1", BTN.R1],
      ["l2", BTN.L2],
      ["r2", BTN.R2],
      ["dpad_up", BTN.DPAD_UP],
      ["dpad_down", BTN.DPAD_DOWN],
      ["dpad_left", BTN.DPAD_LEFT],
      ["dpad_right", BTN.DPAD_RIGHT],
      ["options", BTN.OPTIONS],
      ["create", BTN.CREATE],
    ];
    for (const [name, bit] of cases) {
      const state = sampleTouch(0, { active: false, dx: 0, dy: 0 }, { active: false, dx: 0, dy: 0 }, new Set([name]));
      expect(state.buttons & bit, name).toBeTruthy();
    }
  });

  it("maps full deflection to the axis extremes", () => {
    const left = { active: true, dx: -TOUCH_RADIUS, dy: 0 };
    const right = { active: true, dx: 0, dy: TOUCH_RADIUS };
    const state = sampleTouch(2, left, right, new Set());
    expect(state.lx).toBe(0);
    expect(state.ly).toBe(128);
    expect(state.rx).toBe(128);
    expect(state.ry).toBe(255);
  });

  it("clamps deflection beyond the radius", () => {
    // dx=3R, dy=4R → magnitude 5R clamps to R along the same (3,4) direction.
    const left = { active: true, dx: TOUCH_RADIUS * 3, dy: TOUCH_RADIUS * 4 };
    const state = sampleTouch(3, left, { active: false, dx: 0, dy: 0 }, new Set());
    // 128 + (3/5)*127 = 204.2 → 204, 128 + (4/5)*127 = 229.6 → 230
    expect(state.lx).toBe(204);
    expect(state.ly).toBe(230);
  });

  it("saturates a pure-axis full deflection at the extreme", () => {
    const left = { active: true, dx: TOUCH_RADIUS, dy: 0 };
    const right = { active: true, dx: 0, dy: TOUCH_RADIUS };
    const state = sampleTouch(5, left, right, new Set());
    expect(state.lx).toBe(255);
    expect(state.rx).toBe(128);
    expect(state.ry).toBe(255);
  });

  it("sets trigger value to max when L2/R2 are held", () => {
    const state = sampleTouch(4, { active: false, dx: 0, dy: 0 }, { active: false, dx: 0, dy: 0 }, new Set(["l2", "r2"]));
    expect(state.l2).toBe(255);
    expect(state.r2).toBe(255);
  });

  it("encodes to the same 31-byte CLPD wire format as a real pad", () => {
    const state: PadState = sampleTouch(
      7,
      { active: true, dx: TOUCH_RADIUS, dy: 0 },
      { active: true, dx: 0, dy: -TOUCH_RADIUS },
      new Set(["cross", "r2"])
    );
    const buf = new Uint8Array(encodeClpd(state));
    expect(buf.length).toBe(31);
    expect(buf[0]).toBe(0x43);
    expect(buf[1]).toBe(0x4c);
    expect(buf[2]).toBe(0x50);
    expect(buf[3]).toBe(0x44);
    expect(buf[5]).toBe(7);
    expect(buf[13]).toBe(255); // lx
    expect(buf[16]).toBe(0);   // ry (up)
    expect(state.buttons & BTN.CROSS).toBeTruthy();
    expect(state.buttons & BTN.R2).toBeTruthy();
  });
});